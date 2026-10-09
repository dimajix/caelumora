use caelumora_model::agent::AgentId;
use caelumora_model::channel::ChannelId;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::{ActionType, Secured, SecurityAttributes, SecurityContext, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use dashmap::DashMap;
use tracing::warn;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{broadcast, mpsc};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionTopic {
    All,
    Session(SessionId),
    Channel(ChannelId),
    Agent(AgentId),
    User(UserId),
}

/// How many events a subscriber may fall behind before it loses the oldest.
/// A power of two, because a broadcast channel rounds its capacity up to one.
const SUBSCRIPTION_CAPACITY: usize = 128;

/// An event on its way through the bus, with the attributes of the entity
/// it is about. Only the bus sees the attributes: subscribers receive the
/// `GlobalEvent` alone.
#[derive(Debug, Clone)]
struct PublishedEvent {
    event: GlobalEvent,
    attributes: SecurityAttributes,
}

impl Secured for PublishedEvent {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.attributes
    }
}

impl PublishedEvent {
    /// What a party must be allowed to receive this event: to read the
    /// entity it is about - the same question the `EntityStore` asks. A
    /// channel's events go by the channel's attributes, which there is no
    /// action of its own for.
    fn read_action(&self) -> ActionType<'_> {
        let attributes = self.security_attributes();
        match self.event {
            GlobalEvent::AgentCreated(..) | GlobalEvent::AgentUpdated(..) | GlobalEvent::AgentDeleted(..) | GlobalEvent::AgentRestored(..) => ActionType::GetAgent(attributes),
            GlobalEvent::UserCreated(_) | GlobalEvent::UserUpdated(..) | GlobalEvent::UserDeleted(_) => ActionType::GetUser(attributes),
            GlobalEvent::UserProfileUpdated(..) => ActionType::GetUserProfile(attributes),
            GlobalEvent::WorkspaceCreated(_) | GlobalEvent::WorkspaceUpdated(..) | GlobalEvent::WorkspaceDeleted(_) | GlobalEvent::WorkspaceRestored(_) => ActionType::GetWorkspace(attributes),
            GlobalEvent::SkillCreated(_) | GlobalEvent::SkillUpdated(..) | GlobalEvent::SkillDeleted(_) | GlobalEvent::SkillRestored(_) => ActionType::GetSkill(attributes),
            GlobalEvent::ArtifactPublished(..) | GlobalEvent::ArtifactUpdated(_) | GlobalEvent::ArtifactDeleted(_) | GlobalEvent::ArtifactRestored(_) => ActionType::GetArtifact(attributes),
            _ => ActionType::GetSession(attributes),
        }
    }
}

/// A subscription made with `subscribe_as`: the context it receives for, and
/// where its events go.
#[derive(Debug)]
struct Party {
    context: SecurityContext,
    tx: broadcast::Sender<GlobalEvent>,
}

#[derive(Debug)]
pub struct EventBus {
    // One sender per subscription, registered under each of its topics. A
    // broadcast channel rather than an mpsc one: when a subscriber falls
    // behind, it is its receiver that learns so (`Lagged`), not only the log
    // (issue 38).
    subscribers: DashMap<SubscriptionTopic, Vec<broadcast::Sender<GlobalEvent>>>,
    // The subscriptions made with `subscribe_as`, served by one dispatcher
    // task: the policy is asked asynchronously, and `publish` must not wait
    // for it. One task, so that every party receives the events in the
    // order they were published.
    parties: Arc<Mutex<Vec<Party>>>,
    dispatcher: OnceLock<mpsc::UnboundedSender<PublishedEvent>>,
    security_manager: Arc<dyn SecurityManager>,
}

/// The events of an `EventBus` subscription.
///
/// A subscriber that falls more than `SUBSCRIPTION_CAPACITY` events behind
/// loses the oldest ones. `recv` notes that in the log and goes on, for
/// subscribers that have no way to catch up; one that can - a stream that
/// sends its client a fresh snapshot - takes the receiver with
/// `into_receiver` and handles `RecvError::Lagged` itself.
#[derive(Debug)]
pub struct EventSubscription {
    rx: broadcast::Receiver<GlobalEvent>,
}

impl EventSubscription {
    /// The next event, or `None` once the bus is gone. Lost events are
    /// logged and skipped.
    pub async fn recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(lost)) => warn!("An event bus subscriber fell behind and lost {} events", lost),
                Err(RecvError::Closed) => return None,
            }
        }
    }

    /// The next event if one is waiting, like `recv`.
    pub fn try_recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Lagged(lost)) => warn!("An event bus subscriber fell behind and lost {} events", lost),
                Err(TryRecvError::Empty | TryRecvError::Closed) => return None,
            }
        }
    }

    /// The underlying receiver, for a subscriber that handles lost events
    /// itself.
    pub fn into_receiver(self) -> broadcast::Receiver<GlobalEvent> {
        self.rx
    }
}

impl EventPublisher for EventBus {
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
        self.publish(event, attributes);
    }
}

impl EventBus {
    /// A bus whose `subscribe_as` subscriptions receive what
    /// `security_manager` lets their context read.
    pub fn new(security_manager: Arc<dyn SecurityManager>) -> Self {
        Self {
            subscribers: DashMap::new(),
            parties: Arc::new(Mutex::new(Vec::new())),
            dispatcher: OnceLock::new(),
            security_manager,
        }
    }

    /// The events `context` may receive: those about entities it may read,
    /// as the `SecurityManager` decides - the same decision the
    /// `EntityStore` makes for reading them (issue 39). Nothing else reaches
    /// the subscription, so other parties' events take no room in it.
    ///
    /// Must be called within a Tokio runtime: the first such subscription
    /// starts the task that serves all of them.
    pub fn subscribe_as(&self, context: SecurityContext) -> EventSubscription {
        let (tx, rx) = broadcast::channel(SUBSCRIPTION_CAPACITY);
        if let Ok(mut parties) = self.parties.lock() {
            parties.push(Party { context, tx });
        }
        self.dispatcher.get_or_init(|| self.start_dispatcher());
        EventSubscription { rx }
    }

    fn start_dispatcher(&self) -> mpsc::UnboundedSender<PublishedEvent> {
        let (tx, mut rx) = mpsc::unbounded_channel::<PublishedEvent>();
        let parties = Arc::clone(&self.parties);
        let security_manager = Arc::clone(&self.security_manager);
        tokio::spawn(async move {
            while let Some(published) = rx.recv().await {
                // The parties as they are now; the lock is not held while the
                // policy is asked.
                let current: Vec<(SecurityContext, broadcast::Sender<GlobalEvent>)> = match parties.lock() {
                    Ok(parties) => parties.iter().map(|p| (p.context.clone(), p.tx.clone())).collect(),
                    Err(_) => continue,
                };
                let mut gone = Vec::new();
                for (context, tx) in current {
                    if !security_manager.check(&context, published.read_action()).await {
                        continue;
                    }
                    // A send fails only when the subscription is gone.
                    if tx.send(published.event.clone()).is_err() {
                        gone.push(tx);
                    }
                }
                if !gone.is_empty() {
                    if let Ok(mut parties) = parties.lock() {
                        parties.retain(|p| !gone.iter().any(|g| g.same_channel(&p.tx)));
                    }
                }
            }
        });
        tx
    }

    pub fn subscribe(&self, topics: Vec<SubscriptionTopic>) -> EventSubscription {
        let (tx, rx) = broadcast::channel(SUBSCRIPTION_CAPACITY);

        for topic in topics {
            self.subscribers.entry(topic).or_default().push(tx.clone());
        }
        EventSubscription { rx }
    }

    pub fn subscribe_all(&self) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::All])
    }

    pub fn subscribe_session(&self, id: SessionId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Session(id)])
    }

    pub fn subscribe_channel(&self, id: ChannelId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Channel(id)])
    }

    pub fn subscribe_agent(&self, id: AgentId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Agent(id)])
    }

    pub fn subscribe_user(&self, id: UserId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::User(id)])
    }

    /// Publishes `event` about an entity with these `attributes` (see
    /// `EventPublisher::publish`): at once to the subscribers of its topics,
    /// and through the dispatcher to every `subscribe_as` subscription
    /// allowed to read that entity.
    pub fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
        if let Some(dispatcher) = self.dispatcher.get() {
            let _ = dispatcher.send(PublishedEvent { event: event.clone(), attributes });
        }

        let mut topics_to_notify = vec![SubscriptionTopic::All];

        match &event {
            GlobalEvent::SessionCreated(sid) | GlobalEvent::SessionUpdated(sid, _) | GlobalEvent::SessionDeleted(sid) | GlobalEvent::SessionRestored(sid) |
            GlobalEvent::SessionStarted(sid) | GlobalEvent::SessionStopped(sid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
            }
            GlobalEvent::ChannelCreated(sid, cid) | GlobalEvent::ChannelUpdated(sid, cid, _) | GlobalEvent::ChannelDeleted(sid, cid) | GlobalEvent::ChannelRestored(sid, cid) |
            GlobalEvent::ChannelStarted(sid, cid) | GlobalEvent::ChannelStopped(sid, cid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
            }
            GlobalEvent::AgentCreated(aid) | GlobalEvent::AgentUpdated(aid, _) | GlobalEvent::AgentDeleted(aid) | GlobalEvent::AgentRestored(aid) => {
                topics_to_notify.push(SubscriptionTopic::Agent(*aid));
            }
            GlobalEvent::AgentStateChanged(aid) => {
                topics_to_notify.push(SubscriptionTopic::Agent(*aid));
            }
            GlobalEvent::AgentChannelStateChanged(sid, cid, aid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
                topics_to_notify.push(SubscriptionTopic::Agent(*aid));
            }
            GlobalEvent::UserCreated(uid) | GlobalEvent::UserUpdated(uid, _) | GlobalEvent::UserDeleted(uid) | GlobalEvent::UserProfileUpdated(uid, _) => {
                topics_to_notify.push(SubscriptionTopic::User(*uid));
            }
            GlobalEvent::AgentChannelJoined(sid, cid, _) | GlobalEvent::AgentChannelLeft(sid, cid, _) |
            GlobalEvent::UserChannelJoined(sid, cid, _) | GlobalEvent::UserChannelLeft(sid, cid, _) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
            }
            // No topic of their own until something follows one workspace
            // (issue 253): `All`, and whoever may read it.
            GlobalEvent::WorkspaceCreated(_) | GlobalEvent::WorkspaceUpdated(..) | GlobalEvent::WorkspaceDeleted(_) | GlobalEvent::WorkspaceRestored(_) => {}
            // Likewise for skills (issue 22).
            GlobalEvent::SkillCreated(_) | GlobalEvent::SkillUpdated(..) | GlobalEvent::SkillDeleted(_) | GlobalEvent::SkillRestored(_) => {}
            // Likewise for artifacts (issue 271).
            GlobalEvent::ArtifactPublished(..) | GlobalEvent::ArtifactUpdated(_) | GlobalEvent::ArtifactDeleted(_) | GlobalEvent::ArtifactRestored(_) => {}
        }

        for topic in topics_to_notify {
            if let Some(mut senders) = self.subscribers.get_mut(&topic) {
                // A full channel does not fail the send: it drops the
                // subscriber's oldest event and tells its receiver. A send
                // fails only when the receiver is gone, and so does its
                // subscription.
                senders.retain(|tx| tx.send(event.clone()).is_ok());
            }
        }

        // Periodic cleanup of empty topics could be added here, but DashMap doesn't make it as easy as retain on Mutex<HashMap>.
        // For now, let's keep it simple. Empty Vecs will remain in the DashMap.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::{DummySecurityManager, OwnershipSecurityManager};
    use caelumora_model::actor::ActorId;
    use std::time::Duration;

    fn bus() -> EventBus {
        EventBus::new(Arc::new(DummySecurityManager::new()))
    }

    fn created() -> GlobalEvent {
        GlobalEvent::SessionCreated(SessionId::new())
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_is_told_and_keeps_receiving() {
        let bus = bus();
        let mut receiver = bus.subscribe_all().into_receiver();
        for _ in 0..SUBSCRIPTION_CAPACITY + 5 {
            bus.publish(created(), Default::default());
        }

        assert!(matches!(receiver.recv().await, Err(RecvError::Lagged(5))), "the loss reaches the subscriber");
        assert!(receiver.recv().await.is_ok(), "and the newest events are still there");
    }

    #[tokio::test]
    async fn a_subscription_skips_what_it_lost_and_goes_on() {
        let bus = bus();
        let mut subscription = bus.subscribe_all();
        for _ in 0..SUBSCRIPTION_CAPACITY + 5 {
            bus.publish(created(), Default::default());
        }
        let deleted = SessionId::new();
        bus.publish(GlobalEvent::SessionDeleted(deleted), Default::default());

        let mut received = 0;
        while let Some(event) = subscription.try_recv() {
            received += 1;
            if matches!(event, GlobalEvent::SessionDeleted(id) if id == deleted) {
                break;
            }
        }
        assert_eq!(received, SUBSCRIPTION_CAPACITY, "the oldest events are lost, the rest arrives in order");
    }

    #[tokio::test]
    async fn a_dropped_subscription_is_removed() {
        let bus = bus();
        drop(bus.subscribe_all());
        bus.publish(created(), Default::default());
        assert!(bus.subscribers.get(&SubscriptionTopic::All).unwrap().is_empty());
    }

    fn as_user(id: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
    }

    /// Everything that arrives within a short while.
    async fn drain(subscription: &mut EventSubscription) -> Vec<GlobalEvent> {
        let mut events = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(100), subscription.recv()).await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn a_party_receives_what_it_may_read_and_nothing_else() {
        let bus = EventBus::new(Arc::new(OwnershipSecurityManager::new()));
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));
        let mut carols = bus.subscribe_as(as_user(carol));

        let hers = SecurityAttributes { owner_id: ActorId::User(alice), contributors: vec![ActorId::User(bob)] };
        let (session, deleted) = (SessionId::new(), SessionId::new());
        bus.publish(GlobalEvent::SessionCreated(session), hers.clone());
        bus.publish(GlobalEvent::ChannelCreated(session, ChannelId::new()), hers.clone());
        bus.publish(GlobalEvent::SessionDeleted(deleted), hers.clone());
        bus.publish(GlobalEvent::UserCreated(carol), SecurityAttributes::owned_by(ActorId::User(carol)));

        let kinds = |events: Vec<GlobalEvent>| events.iter().map(|e| format!("{e:?}").split('(').next().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(kinds(drain(&mut alices).await), ["SessionCreated", "ChannelCreated", "SessionDeleted"], "the owner gets it all, in order - a deletion too");
        assert_eq!(kinds(drain(&mut bobs).await), ["SessionCreated", "ChannelCreated", "SessionDeleted"], "so does a contributor");
        assert_eq!(kinds(drain(&mut carols).await), ["UserCreated"], "a stranger only her own user event");
    }

    #[tokio::test]
    async fn a_profile_s_change_reaches_its_user_alone() {
        let bus = EventBus::new(Arc::new(OwnershipSecurityManager::new()));
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));

        let profile = caelumora_model::user::UserProfile::new(alice, Some(caelumora_model::value::text::Introduction::new("I run the budget.").unwrap()));
        let attributes = caelumora_model::security::Secured::security_attributes(&profile).clone();
        bus.publish(GlobalEvent::UserProfileUpdated(alice, profile), attributes);

        assert_eq!(drain(&mut alices).await.len(), 1, "hers");
        assert!(drain(&mut bobs).await.is_empty(), "not his, although her user record is everybody's");
    }

    #[tokio::test]
    async fn a_workspace_s_events_reach_whoever_may_use_it() {
        let bus = EventBus::new(Arc::new(OwnershipSecurityManager::new()));
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));
        let mut carols = bus.subscribe_as(as_user(carol));
        let mut everything = bus.subscribe_all();

        let mut workspace = caelumora_model::workspace::Workspace::new(caelumora_model::value::name::ScopedName::new("docs", caelumora_model::scope::Scope::UserScope { user_id: alice }).unwrap(), "main", Default::default(), alice);
        workspace.security_attributes.contributors.push(ActorId::User(bob));
        let attributes = workspace.security_attributes.clone();
        bus.publish(GlobalEvent::WorkspaceCreated(workspace.id), attributes.clone());
        bus.publish(GlobalEvent::WorkspaceDeleted(workspace.id), attributes);

        assert_eq!(drain(&mut alices).await.len(), 2, "its owner");
        assert_eq!(drain(&mut bobs).await.len(), 2, "a contributor");
        assert!(drain(&mut carols).await.is_empty(), "nobody else");
        assert_eq!(drain(&mut everything).await.len(), 2, "and the runtime, on `All`");
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_reaches_every_party() {
        let bus = EventBus::new(Arc::new(OwnershipSecurityManager::new()));
        let mut someone = bus.subscribe_as(as_user(UserId::new()));
        bus.publish(GlobalEvent::SessionCreated(SessionId::new()), SecurityAttributes::default());
        assert_eq!(drain(&mut someone).await.len(), 1);
    }

    #[tokio::test]
    async fn a_dropped_party_is_removed() {
        let bus = EventBus::new(Arc::new(DummySecurityManager::new()));
        drop(bus.subscribe_as(as_user(UserId::new())));
        let mut kept = bus.subscribe_as(as_user(UserId::new()));
        bus.publish(GlobalEvent::SessionCreated(SessionId::new()), SecurityAttributes::default());
        drain(&mut kept).await;
        assert_eq!(bus.parties.lock().unwrap().len(), 1);
    }
}
