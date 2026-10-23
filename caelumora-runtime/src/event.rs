use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::{may_reach, Classification, ConfidentialityPolicy};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::{ActionType, Secured, SecurityAttributes, SecurityContext, AccessPolicy};
use caelumora_model::session::SessionId;
use caelumora_model::user::{User, UserId, UserProfile};
use dashmap::DashMap;
use tracing::warn;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{broadcast, mpsc};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use caelumora_model::channel::Channel;
use caelumora_model::session::Session;

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
/// it is about - and of the part of it, if it is about one - and, for a
/// classified one, its classifications. Only the bus sees them: subscribers
/// receive the `GlobalEvent` alone.
#[derive(Debug, Clone)]
struct PublishedEvent {
    event: GlobalEvent,
    attributes: SecurityAttributes,
    part: Option<SecurityAttributes>,
    classifications: Vec<Classification>,
}

impl Secured for PublishedEvent {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.attributes
    }
}

impl PublishedEvent {
    /// What a party must be allowed to receive this event: to read the
    /// entity it is about - the same question the `EntityStore` asks. What
    /// happens to a channel's participants and to an agent's state in it is an
    /// event of the channel, and goes by its attributes (issue 304).
    fn read_action(&self) -> ActionType<'_> {
        ActionType::get(self.event.kind, self.security_attributes())
    }

    /// Whether `context` may read the part the event is about, besides the
    /// entity (issue 316): asked as reading the entity is, with the part's
    /// attributes. Nothing to ask of an event about the whole.
    async fn part_readable(&self, access_policy: &dyn AccessPolicy, context: &SecurityContext) -> bool {
        match &self.part {
            Some(part) => access_policy.check(context, ActionType::get(self.event.kind, part)).await,
            None => true,
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
    access_policy: Arc<dyn AccessPolicy>,
    /// Who is cleared for what a classified entity's events are about
    /// (issue 312).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

/// The events of an `EventBus` subscription.
///
/// A subscriber that falls more than `SUBSCRIPTION_CAPACITY` events behind
/// loses the oldest ones. `recv` notes that in the log and goes on, for
/// subscribers that have no way to catch up; one that can - a stream that
/// sends its client a fresh snapshot - takes the receiver with
/// `into_receiver` and handles `RecvError::Lagged` itself, or borrows it
/// with `receiver` to keep the subscription, which `EventBus::follow`
/// extends by a topic.
#[derive(Debug)]
pub struct EventSubscription {
    rx: broadcast::Receiver<GlobalEvent>,
    /// What the bus sends its events with under each of its topics; `None`
    /// for one made by `subscribe_as`, whose events the policy filters and
    /// which follows no topic.
    tx: Option<broadcast::Sender<GlobalEvent>>,
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

    /// The underlying receiver, lent to a subscriber that handles lost
    /// events itself and keeps the subscription to follow more topics.
    pub fn receiver(&mut self) -> &mut broadcast::Receiver<GlobalEvent> {
        &mut self.rx
    }
}

impl EventPublisher for EventBus {
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
        self.publish(event, attributes);
    }

    fn publish_classified(&self, event: GlobalEvent, attributes: SecurityAttributes, classifications: &[Classification]) {
        self.publish_classified(event, attributes, classifications);
    }
}

impl EventBus {
    /// A bus whose `subscribe_as` subscriptions receive what
    /// `access_policy` lets their context read and, of what is
    /// classified, what `confidentiality` clears them for.
    pub fn new(access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> Self {
        Self {
            subscribers: DashMap::new(),
            parties: Arc::new(Mutex::new(Vec::new())),
            dispatcher: OnceLock::new(),
            access_policy,
            confidentiality,
        }
    }

    /// The events `context` may receive: those about entities it may read,
    /// as the `AccessPolicy` decides - the same decision the
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
        EventSubscription { rx, tx: None }
    }

    fn start_dispatcher(&self) -> mpsc::UnboundedSender<PublishedEvent> {
        let (tx, mut rx) = mpsc::unbounded_channel::<PublishedEvent>();
        let parties = Arc::clone(&self.parties);
        let access_policy = Arc::clone(&self.access_policy);
        let confidentiality = Arc::clone(&self.confidentiality);
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
                    // Reached as the store reaches it: read, and cleared for
                    // what it is, or was (issues 312, 322).
                    if !may_reach(access_policy.as_ref(), confidentiality.as_ref(), &context, published.read_action(), &published.classifications).await
                        || !published.part_readable(access_policy.as_ref(), &context).await {
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
        EventSubscription { rx, tx: Some(tx) }
    }

    /// Has `subscription`, made by `subscribe`, receive the events of
    /// `topic` from now on, besides those of its topics - a running session
    /// those of an agent taken in (issue 221). Nothing for one it follows
    /// already, nor for one made by `subscribe_as`, which no topic reaches
    /// past the policy.
    pub fn follow(&self, subscription: &EventSubscription, topic: SubscriptionTopic) {
        let Some(tx) = &subscription.tx else { return };
        let mut senders = self.subscribers.entry(topic).or_default();
        if !senders.iter().any(|sender| sender.same_channel(tx)) {
            senders.push(tx.clone());
        }
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
        self.publish_classified(event, attributes, &[]);
    }

    /// Publishes `event` about an entity with these `attributes`, classified
    /// as `classifications` says (see `EventPublisher::publish_classified`):
    /// as `publish` does, and to a `subscribe_as` subscription only if it
    /// is cleared for one of them.
    pub fn publish_classified(&self, event: GlobalEvent, attributes: SecurityAttributes, classifications: &[Classification]) {
        self.dispatch(event, attributes, None, classifications);
    }

    /// Publishes `event` about a part of a classified entity with these
    /// `attributes` - a version of an artifact (issue 316) -: as
    /// `publish_classified` does, and to a `subscribe_as` subscription only
    /// if it may read the part, with `part`, as well. Reading a part needs
    /// both, as a file in HDFS needs its directory.
    pub fn publish_classified_part(&self, event: GlobalEvent, attributes: SecurityAttributes, part: SecurityAttributes, classifications: &[Classification]) {
        self.dispatch(event, attributes, Some(part), classifications);
    }

    /// Hands `event` to the dispatcher with whom it is about, and to the
    /// subscribers of its topics.
    fn dispatch(&self, event: GlobalEvent, attributes: SecurityAttributes, part: Option<SecurityAttributes>, classifications: &[Classification]) {
        if let Some(dispatcher) = self.dispatcher.get() {
            let _ = dispatcher.send(PublishedEvent { event: event.clone(), attributes, part, classifications: classifications.to_vec() });
        }

        let mut topics_to_notify = vec![SubscriptionTopic::All];

        if let Some((sid, _)) = event.about::<Session>() {
            topics_to_notify.push(SubscriptionTopic::Session(sid));
        } else if let Some(((sid, cid), _)) = event.about::<Channel>() {
            // A channel's events, and those of its participants, reach the
            // session it lies in too (issue 335).
            topics_to_notify.push(SubscriptionTopic::Session(sid));
            topics_to_notify.push(SubscriptionTopic::Channel(cid));
        } else if let Some((aid, _)) = event.about::<Agent>() {
            topics_to_notify.push(SubscriptionTopic::Agent(aid));
        } else if let Some((uid, _)) = event.about::<User>().or_else(|| event.about::<UserProfile>()) {
            topics_to_notify.push(SubscriptionTopic::User(uid));
        }
        // Other kinds have no topic of their own until something follows
        // one of them (issues 253, 22, 271, 286): `All`, and whoever may
        // read it.

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
    use caelumora_model::security::{AllowAllPolicy, OwnershipPolicy, AclEntry};
    use caelumora_model::actor::ActorId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use std::time::Duration;
    use caelumora_model::channel::{self, ChannelUpdateEvent};
    use caelumora_model::security::{Grant, Subject};
    use caelumora_model::event::EntityChange;
    use caelumora_model::store::Stored;
    use caelumora_model::workspace::{Workspace, WorkspaceId};
    use caelumora_model::scope::Scope;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::value::text::Introduction;
    use std::slice;
    use tokio::time;

    fn bus() -> EventBus {
        EventBus::new(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()))
    }

    fn created() -> GlobalEvent {
        GlobalEvent::created::<Session>(&SessionId::new())
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
        bus.publish(GlobalEvent::deleted::<Session>(&deleted), Default::default());

        let mut received = 0;
        while let Some(event) = subscription.try_recv() {
            received += 1;
            if event.about::<Session>() == Some((deleted, &EntityChange::Deleted)) {
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

    /// What `events` are about: the kind, and the change or the name of a
    /// custom one.
    fn described(events: Vec<GlobalEvent>) -> Vec<String> {
        events.iter().map(|event| match &event.change {
            EntityChange::Custom { name, .. } => format!("{} {name}", event.kind),
            EntityChange::Updated(_) => format!("{} Updated", event.kind),
            other => format!("{} {other:?}", event.kind),
        }).collect()
    }

    /// Issue 335: a channel's events, and those of its participants, reach the
    /// session it lies in and the channel, by what the event names.
    #[tokio::test]
    async fn a_channel_s_events_reach_its_session_and_itself() {
        let bus = bus();
        let (session, channel, other) = (SessionId::new(), ChannelId::new(), ChannelId::new());
        let mut sessions = bus.subscribe_session(session);
        let mut channels = bus.subscribe_channel(channel);
        let mut others = bus.subscribe_channel(other);
        let agent = AgentId::new();
        bus.publish(GlobalEvent::custom::<Channel>(&(session, channel), channel::JOINED, &ActorId::Agent(agent)), Default::default());
        bus.publish(GlobalEvent::custom::<Channel>(&(session, channel), channel::AGENT_STATE_CHANGED, &agent), Default::default());

        assert_eq!(described(drain(&mut sessions).await), ["channel joined", "channel agent_state_changed"], "its session");
        assert_eq!(described(drain(&mut channels).await), ["channel joined", "channel agent_state_changed"], "the channel");
        assert!(drain(&mut others).await.is_empty(), "no other channel");
    }

    /// Everything that arrives within a short while.
    async fn drain(subscription: &mut EventSubscription) -> Vec<GlobalEvent> {
        let mut events = Vec::new();
        while let Ok(Some(event)) = time::timeout(Duration::from_millis(100), subscription.recv()).await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn a_party_receives_what_it_may_read_and_nothing_else() {
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));
        let mut carols = bus.subscribe_as(as_user(carol));

        let hers = SecurityAttributes::owned_by(ActorId::User(alice)).granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let (session, deleted) = (SessionId::new(), SessionId::new());
        bus.publish(GlobalEvent::created::<Session>(&session), hers.clone());
        bus.publish(GlobalEvent::created::<Channel>(&(session, ChannelId::new())), hers.clone());
        bus.publish(GlobalEvent::deleted::<Session>(&deleted), hers.clone());
        bus.publish(GlobalEvent::created::<User>(&carol), SecurityAttributes::owned_by(ActorId::User(carol)));

        assert_eq!(described(drain(&mut alices).await), ["session Created", "channel Created", "session Deleted"], "the owner gets it all, in order - a deletion too");
        assert_eq!(described(drain(&mut bobs).await), ["session Created", "channel Created", "session Deleted"], "so does a contributor");
        assert_eq!(described(drain(&mut carols).await), ["user Created"], "a stranger only her own user event");
    }

    #[tokio::test]
    async fn the_events_of_a_channel_are_received_by_who_may_read_the_channel() {
        use caelumora_model::security::Verb;

        /// Lets read channels alone, so that the question asked shows.
        #[derive(Debug)]
        struct ChannelsOnly;

        #[async_trait::async_trait]
        impl AccessPolicy for ChannelsOnly {
            async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
                matches!(action, ActionType::Entity { kind: Channel::KIND, verb: Verb::Get, .. })
            }
        }

        let bus = EventBus::new(Arc::new(ChannelsOnly), Arc::new(ConfidentialityRules::default()));
        let alice = UserId::new();
        let mut alices = bus.subscribe_as(as_user(alice));
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let attributes = SecurityAttributes::owned_by(ActorId::User(alice));
        bus.publish(GlobalEvent::created::<Session>(&session), attributes.clone());
        bus.publish(GlobalEvent::created::<Channel>(&(session, channel)), attributes.clone());
        bus.publish(GlobalEvent::custom::<Channel>(&(session, channel), channel::JOINED, &ActorId::User(alice)), attributes.clone());
        bus.publish(GlobalEvent::custom::<Channel>(&(session, channel), channel::AGENT_STATE_CHANGED, &AgentId::new()), attributes);

        assert_eq!(described(drain(&mut alices).await), ["channel Created", "channel joined", "channel agent_state_changed"], "asked as reading the channel (issue 304), the session's as reading it");
    }

    #[tokio::test]
    async fn the_events_of_a_classified_channel_reach_who_is_cleared_for_it() {
        use caelumora_model::confidentiality::{Clearance, Compartments, Confidentiality};
        use caelumora_model::role::{RoleId, Roles};
        let staff = RoleId::new();
        let roles = Roles::new([(staff, "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(rules));
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice).with_roles(vec![staff], None));
        let mut bobs = bus.subscribe_as(as_user(bob));

        let (session, channel) = (SessionId::new(), ChannelId::new());
        let shared = SecurityAttributes::owned_by(ActorId::User(alice)).granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let (public, internal) = (Classification::general(Confidentiality::Public), Classification::general(Confidentiality::Internal));
        bus.publish_classified(GlobalEvent::created::<Channel>(&(session, channel)), shared.clone(), slice::from_ref(&internal));
        bus.publish_classified(GlobalEvent::custom::<Channel>(&(session, channel), channel::STARTED, &()), shared.clone(), slice::from_ref(&public));
        let lowered = ChannelUpdateEvent::ClassificationChanged(public.clone());
        bus.publish_classified(GlobalEvent::updated::<Channel>(&(session, channel), &lowered), shared, &[internal, public]);

        assert_eq!(described(drain(&mut alices).await), ["channel Created", "channel started", "channel Updated"], "cleared, she gets it all");
        assert_eq!(described(drain(&mut bobs).await), ["channel started", "channel Updated"], "he reads it, but is cleared only for what is public (issue 312) - and learns of its reclassification");
    }

    #[tokio::test]
    async fn a_profile_s_change_reaches_its_user_alone() {
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));

        let profile = UserProfile::new(alice, Some(Introduction::new("I run the budget.").unwrap()));
        let attributes = Secured::security_attributes(&profile).clone();
        bus.publish(GlobalEvent::updated::<UserProfile>(&alice, &profile), attributes);

        assert_eq!(drain(&mut alices).await.len(), 1, "hers");
        assert!(drain(&mut bobs).await.is_empty(), "not his, although her user record is everybody's");
    }

    /// An event about a part of an entity - a version of an artifact (issue
    /// 316) - reaches whoever may read both the entity and the part.
    #[tokio::test]
    async fn a_part_s_events_reach_whoever_reads_the_entity_and_the_part() {
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let (alice, bob, carol, dave) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));
        let mut carols = bus.subscribe_as(as_user(carol));
        let mut daves = bus.subscribe_as(as_user(dave));

        let reads = |user| Subject::Actor(ActorId::User(user));
        let whole = SecurityAttributes::owned_by(ActorId::User(alice)).granting(reads(bob), Grant::Read).granting(reads(carol), Grant::Read);
        let part = SecurityAttributes::owned_by(ActorId::User(alice)).granting(reads(bob), Grant::Read).granting(reads(dave), Grant::Read);
        bus.publish_classified_part(GlobalEvent::created::<Workspace>(&WorkspaceId::new()), whole, part, &[]);

        assert_eq!(drain(&mut alices).await.len(), 1, "its owner");
        assert_eq!(drain(&mut bobs).await.len(), 1, "who reads both");
        assert!(drain(&mut carols).await.is_empty(), "not who reads the entity alone");
        assert!(drain(&mut daves).await.is_empty(), "nor who reads the part alone");
    }

    #[tokio::test]
    async fn a_workspace_s_events_reach_whoever_may_use_it() {
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe_as(as_user(alice));
        let mut bobs = bus.subscribe_as(as_user(bob));
        let mut carols = bus.subscribe_as(as_user(carol));
        let mut everything = bus.subscribe_all();

        let mut workspace = Workspace::new(ScopedName::new("docs", Scope::UserScope { user_id: alice }).unwrap(), "main", Default::default(), alice);
        workspace.security_attributes.acl.push(AclEntry { subject: Subject::Actor(ActorId::User(bob)), grant: Grant::Read });
        let attributes = workspace.security_attributes.clone();
        bus.publish(GlobalEvent::created::<Workspace>(&workspace.id), attributes.clone());
        bus.publish(GlobalEvent::deleted::<Workspace>(&workspace.id), attributes);

        assert_eq!(drain(&mut alices).await.len(), 2, "its owner");
        assert_eq!(drain(&mut bobs).await.len(), 2, "a contributor");
        assert!(drain(&mut carols).await.is_empty(), "nobody else");
        assert_eq!(drain(&mut everything).await.len(), 2, "and the runtime, on `All`");
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_reaches_every_party() {
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let mut someone = bus.subscribe_as(as_user(UserId::new()));
        bus.publish(GlobalEvent::created::<Session>(&SessionId::new()), SecurityAttributes::default());
        assert_eq!(drain(&mut someone).await.len(), 1);
    }

    #[tokio::test]
    async fn a_dropped_party_is_removed() {
        let bus = EventBus::new(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()));
        drop(bus.subscribe_as(as_user(UserId::new())));
        let mut kept = bus.subscribe_as(as_user(UserId::new()));
        bus.publish(GlobalEvent::created::<Session>(&SessionId::new()), SecurityAttributes::default());
        drain(&mut kept).await;
        assert_eq!(bus.parties.lock().unwrap().len(), 1);
    }
}
