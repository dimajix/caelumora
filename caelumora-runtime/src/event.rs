//! The runtime's event bus (issue 39): what happens to entities, delivered
//! to each subscriber that follows it and may read it, in the order it was
//! published.

use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::channel::Channel;
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::{may_reach_each, Classification, ConfidentialityPolicy};
use caelumora_model::event::{EntityChange, EventPublisher, GlobalEvent};
use caelumora_model::security::{AccessPolicy, ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::session::Session;
use caelumora_model::session::SessionId;
use caelumora_model::user::{User, UserId, UserProfile};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::oneshot;
use tokio::sync::{broadcast, mpsc};
#[cfg(any(test, feature = "test-support"))]
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionTopic {
    All,
    Session(SessionId),
    Channel(ChannelId),
    Agent(AgentId),
    User(UserId),
}

impl SubscriptionTopic {
    /// The topics `event` reaches: everything, and the entity it is about
    /// where that has a topic - a channel's events, and those of its
    /// participants, the session it lies in too (issue 335). Other kinds
    /// have no topic of their own until something follows one of them
    /// (issues 253, 22, 271, 286, 346): `All` alone.
    fn of(event: &GlobalEvent) -> Vec<Self> {
        let mut topics = vec![Self::All];
        if let Some((sid, _)) = event.about::<Session>() {
            topics.push(Self::Session(sid));
        } else if let Some(((sid, cid), _)) = event.about::<Channel>() {
            topics.push(Self::Session(sid));
            topics.push(Self::Channel(cid));
        } else if let Some((aid, _)) = event.about::<Agent>() {
            topics.push(Self::Agent(aid));
        } else if let Some((uid, _)) =
            event.about::<User>().or_else(|| event.about::<UserProfile>())
        {
            topics.push(Self::User(uid));
        }
        topics
    }
}

/// How many events a subscriber may fall behind before it loses the oldest.
/// A power of two, because a broadcast channel rounds its capacity up to one.
const SUBSCRIPTION_CAPACITY: usize = 128;

/// How many events the dispatcher takes from its queue at once: a burst is
/// handed on in one go.
const DISPATCHED_AT_ONCE: usize = 1024;

/// An event on its way through the bus, with the attributes of the entity
/// it is about - and of the part of it, if it is about one - and, for a
/// classified one, its classifications. Only the bus sees them: subscribers
/// receive the `GlobalEvent` alone.
#[derive(Debug, Clone)]
struct PublishedEvent {
    event: Arc<GlobalEvent>,
    attributes: SecurityAttributes,
    part: Option<SecurityAttributes>,
    classifications: Vec<Classification>,
}

impl PublishedEvent {
    /// What a subscriber must be allowed to receive this event, of what has
    /// `attributes` - the entity it is about, or its part: to read it - the
    /// same question the `EntityStore` asks -, or, for its deletion, to have
    /// read it until then (`Verb::HearDeleted`): the groups that reached it
    /// may have gone with it, and are asked after they went, as the bus
    /// delivers. What happens to a channel's participants and to an agent's
    /// state in it is an event of the channel, and goes by its attributes
    /// (issue 304).
    fn action_on<'a>(&self, attributes: &'a SecurityAttributes) -> ActionType<'a> {
        match self.event.change {
            EntityChange::Deleted => ActionType::hear_deleted(self.event.kind, attributes),
            EntityChange::Created
            | EntityChange::Updated(_)
            | EntityChange::Restored
            | EntityChange::Custom { .. } => ActionType::get(self.event.kind, attributes),
        }
    }

    /// Which of `contexts` may receive this event, in their order: who
    /// reaches what it is about as the store reaches it - read, and cleared
    /// for what it is, or was (issues 312, 322) -, and of an event about a
    /// part, who reads the part as well (issue 316). Asked of all of them at
    /// once, so that what the question needs of others is read once per
    /// question (issue 503).
    async fn receivable_by(
        &self,
        access_policy: &dyn AccessPolicy,
        confidentiality: &dyn ConfidentialityPolicy,
        contexts: &[SecurityContext],
    ) -> Vec<bool> {
        let reached = may_reach_each(
            access_policy,
            confidentiality,
            contexts,
            self.action_on(&self.attributes),
            &self.classifications,
        )
        .await;
        let Some(part) = &self.part else { return reached };
        let part = access_policy.check_each(contexts, self.action_on(part)).await;
        reached.into_iter().zip(part).map(|(whole, part)| whole && part).collect()
    }
}

/// What the dispatcher is handed, in the order it is handed it: an event to
/// deliver, or for tests a mark to answer once everything before it is
/// delivered (`EventSubscription::settled`).
#[derive(Debug)]
enum Dispatched {
    Event(PublishedEvent),
    #[cfg(any(test, feature = "test-support"))]
    Settled(oneshot::Sender<()>),
}

/// A subscription as the bus keeps it: whom it receives for, the topics it
/// follows, and where its events go.
#[derive(Debug)]
struct Subscriber {
    context: SecurityContext,
    topics: Vec<SubscriptionTopic>,
    tx: broadcast::Sender<Arc<GlobalEvent>>,
}

/// What delivers the events of a bus (issue 504): its subscriptions, and
/// whom it asks who may receive an event. Shared by the bus, which adds to
/// the subscriptions, and by the one task that delivers what is published,
/// in that order (`run`) - so that the policy is asked asynchronously,
/// `publish` does not wait for it, and every subscriber receives the
/// events in the order they were published.
#[derive(Debug)]
struct Dispatcher {
    subscribers: Mutex<Vec<Subscriber>>,
    access_policy: Arc<dyn AccessPolicy>,
    /// Who is cleared for what a classified entity's events are about
    /// (issue 312).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl Dispatcher {
    /// The subscriptions, held only while they are read or changed, never
    /// while the policy is asked.
    fn subscribers(&self) -> MutexGuard<'_, Vec<Subscriber>> {
        self.subscribers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Has the subscription that sends with `tx` receive the events of
    /// `topic` as well; nothing for one it follows already.
    fn follow(&self, tx: &broadcast::Sender<Arc<GlobalEvent>>, topic: SubscriptionTopic) {
        let mut subscribers = self.subscribers();
        let Some(subscriber) = subscribers.iter_mut().find(|s| s.tx.same_channel(tx)) else {
            return;
        };
        if !subscriber.topics.contains(&topic) {
            subscriber.topics.push(topic);
        }
    }

    /// Delivers what `queue` is handed, in that order, until the bus is
    /// gone. It takes what is queued at once, so that a burst is handed on
    /// in one go.
    async fn run(self: Arc<Self>, mut queue: mpsc::UnboundedReceiver<Dispatched>) {
        let mut queued = Vec::with_capacity(DISPATCHED_AT_ONCE);
        while queue.recv_many(&mut queued, DISPATCHED_AT_ONCE).await > 0 {
            for dispatched in queued.drain(..) {
                match dispatched {
                    Dispatched::Event(published) => self.deliver(&published).await,
                    #[cfg(any(test, feature = "test-support"))]
                    Dispatched::Settled(done) => {
                        let _ = done.send(());
                    }
                }
            }
        }
    }

    /// Delivers `published` to the subscriptions that follow one of its
    /// topics: the runtime's own, subscribed as the system, at once, the
    /// others where the policy lets them receive it, asked of all of them
    /// together. A subscription whose receiver is gone is dropped.
    async fn deliver(&self, published: &PublishedEvent) {
        let topics = SubscriptionTopic::of(&published.event);
        let mut contexts = Vec::new();
        let mut asking = Vec::new();
        {
            let mut subscribers = self.subscribers();
            subscribers.retain(|subscriber| subscriber.tx.receiver_count() > 0);
            for subscriber in subscribers.iter() {
                if !subscriber.topics.iter().any(|topic| topics.contains(topic)) {
                    continue;
                }
                if subscriber.context.is_system() {
                    // A full channel does not fail the send: it drops the
                    // subscriber's oldest event and tells its receiver.
                    let _ = subscriber.tx.send(Arc::clone(&published.event));
                } else {
                    contexts.push(subscriber.context.clone());
                    asking.push(subscriber.tx.clone());
                }
            }
        }
        if contexts.is_empty() {
            return;
        }
        let receiving = published
            .receivable_by(self.access_policy.as_ref(), self.confidentiality.as_ref(), &contexts)
            .await;
        for (tx, receives) in asking.into_iter().zip(receiving) {
            if receives {
                let _ = tx.send(Arc::clone(&published.event));
            }
        }
    }
}

#[derive(Debug)]
pub struct EventBus {
    // A broadcast channel per subscription rather than an mpsc one: when a
    // subscriber falls behind, it is its receiver that learns so
    // (`Lagged`), not only the log (issue 38).
    dispatcher: Arc<Dispatcher>,
    /// What is handed to the dispatcher's task, started with the first
    /// subscription.
    queue: OnceLock<mpsc::UnboundedSender<Dispatched>>,
}

/// The events of an `EventBus` subscription.
///
/// A subscriber that falls more than `SUBSCRIPTION_CAPACITY` events behind
/// loses the oldest ones, and makes up for them (issue 502): it takes the
/// receiver with `into_receiver`, or borrows it with `receiver` to keep the
/// subscription, which `EventBus::follow` extends by a topic, and answers
/// `RecvError::Lagged` by reading anew what it keeps - a stream sends its
/// client a fresh snapshot. Only tests, which read what they caused, skip a
/// loss (`recv`, `try_recv`). An event is made once and shared by every
/// subscriber that receives it (issue 503), so a receiver holds it as an
/// `Arc`.
#[derive(Debug)]
pub struct EventSubscription {
    rx: broadcast::Receiver<Arc<GlobalEvent>>,
    /// What the bus sends its events with: by which `EventBus::follow`
    /// finds the subscription. Weak, so that the receiver learns once the
    /// bus is gone (`Closed`).
    tx: broadcast::WeakSender<Arc<GlobalEvent>>,
    /// The dispatcher's queue, for tests to wait for it (`settled`); weak,
    /// so that a subscription does not keep it running once the bus is
    /// gone.
    #[cfg(any(test, feature = "test-support"))]
    queue: mpsc::WeakUnboundedSender<Dispatched>,
}
impl EventSubscription {
    /// The next event, or `None` once the bus is gone, as a copy of its
    /// own. Lost events are logged and skipped: for tests alone, since a
    /// subscriber of the runtime makes up for them.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(Arc::unwrap_or_clone(event)),
                Err(RecvError::Lagged(lost)) => {
                    warn!("An event bus subscriber fell behind and lost {} events", lost)
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }

    /// The next event if one is waiting, like `recv`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn try_recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => return Some(Arc::unwrap_or_clone(event)),
                Err(TryRecvError::Lagged(lost)) => {
                    warn!("An event bus subscriber fell behind and lost {} events", lost)
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => return None,
            }
        }
    }

    /// Waits until the bus delivered everything published before: for a
    /// test that reads what it caused, since the dispatcher delivers a
    /// moment after `publish` returns (issue 504).
    #[cfg(any(test, feature = "test-support"))]
    pub async fn settled(&self) {
        let Some(queue) = self.queue.upgrade() else { return };
        let (done, settled) = oneshot::channel();
        if queue.send(Dispatched::Settled(done)).is_ok() {
            let _ = settled.await;
        }
    }

    /// Everything delivered of what was published before, as `recv` hands
    /// it out (`settled`): for a test that reads what it caused.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn drain(&mut self) -> Vec<GlobalEvent> {
        self.settled().await;
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        events
    }

    /// The underlying receiver, for a subscriber that handles lost events
    /// itself.
    pub fn into_receiver(self) -> broadcast::Receiver<Arc<GlobalEvent>> {
        self.rx
    }

    /// The underlying receiver, lent to a subscriber that handles lost
    /// events itself and keeps the subscription to follow more topics.
    pub fn receiver(&mut self) -> &mut broadcast::Receiver<Arc<GlobalEvent>> {
        &mut self.rx
    }
}

impl EventPublisher for EventBus {
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
        self.publish(event, attributes);
    }

    fn publish_classified(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        classifications: &[Classification],
    ) {
        self.publish_classified(event, attributes, classifications);
    }
}

impl EventBus {
    /// A bus whose subscriptions receive what `access_policy` lets their
    /// context read and, of what is classified, what `confidentiality`
    /// clears them for.
    pub fn new(
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self {
            dispatcher: Arc::new(Dispatcher {
                subscribers: Mutex::new(Vec::new()),
                access_policy,
                confidentiality,
            }),
            queue: OnceLock::new(),
        }
    }

    /// The events about `topics` that `context` may receive, in the order
    /// they were published - one path for every subscriber. An event is
    /// asked as the `EntityStore` asks reading what it is about (issue 39);
    /// nothing else reaches the subscription, so other subscribers' events
    /// take no room in it. The runtime's own subscribers subscribe as the
    /// system, which the bus does not ask about: they receive everything on
    /// their topics. Who subscribes is told of what the bus has yet to
    /// deliver, also what was published a moment before.
    ///
    /// Must be called within a Tokio runtime: the first subscription starts
    /// the task that serves all of them.
    pub fn subscribe(
        &self,
        context: SecurityContext,
        topics: Vec<SubscriptionTopic>,
    ) -> EventSubscription {
        let (tx, rx) = broadcast::channel(SUBSCRIPTION_CAPACITY);
        let weak = tx.downgrade();
        self.dispatcher.subscribers().push(Subscriber { context, topics, tx });
        #[cfg_attr(
            not(any(test, feature = "test-support")),
            expect(unused_variables, reason = "only tests wait for the dispatcher")
        )]
        let queue = self.queue();
        EventSubscription {
            rx,
            tx: weak,
            #[cfg(any(test, feature = "test-support"))]
            queue: queue.downgrade(),
        }
    }

    /// Has `subscription` receive the events of `topic` as well, of what
    /// the bus has yet to deliver - a running session those of an agent
    /// taken in (issue 221). Nothing for one it follows already.
    pub fn follow(&self, subscription: &EventSubscription, topic: SubscriptionTopic) {
        if let Some(tx) = subscription.tx.upgrade() {
            self.dispatcher.follow(&tx, topic);
        }
    }

    /// Everything, as the system: for a subscriber of the runtime, which
    /// is not asked about (`subscribe`). Must be called within a Tokio
    /// runtime.
    pub fn subscribe_all(&self) -> EventSubscription {
        self.subscribe(SecurityContext::system(), vec![SubscriptionTopic::All])
    }

    /// The events of session `id` and of its channels, as `subscribe_all`.
    pub fn subscribe_session(&self, id: SessionId) -> EventSubscription {
        self.subscribe(SecurityContext::system(), vec![SubscriptionTopic::Session(id)])
    }

    /// The events of channel `id`, as `subscribe_all`.
    pub fn subscribe_channel(&self, id: ChannelId) -> EventSubscription {
        self.subscribe(SecurityContext::system(), vec![SubscriptionTopic::Channel(id)])
    }

    /// What is handed to the dispatcher, whose task is started the first
    /// time.
    fn queue(&self) -> &mpsc::UnboundedSender<Dispatched> {
        self.queue.get_or_init(|| {
            let (queue, handed) = mpsc::unbounded_channel();
            tokio::spawn(Arc::clone(&self.dispatcher).run(handed));
            queue
        })
    }

    /// Publishes `event` about an entity with these `attributes` (see
    /// `EventPublisher::publish`): to every subscription that follows one of
    /// its topics and may read that entity.
    pub fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes) {
        self.publish_classified(event, attributes, &[]);
    }

    /// Publishes `event` about an entity with these `attributes`, classified
    /// as `classifications` says (see `EventPublisher::publish_classified`):
    /// as `publish` does, and only to a subscription cleared for one of
    /// them.
    pub fn publish_classified(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        classifications: &[Classification],
    ) {
        self.dispatch(event, attributes, None, classifications);
    }

    /// Publishes `event` about a part of a classified entity with these
    /// `attributes` - a version of an artifact (issue 316) -: as
    /// `publish_classified` does, and only to a subscription that may read
    /// the part, with `part`, as well. Reading a part needs both, as a file
    /// in HDFS needs its directory.
    pub fn publish_classified_part(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        part: SecurityAttributes,
        classifications: &[Classification],
    ) {
        self.dispatch(event, attributes, Some(part), classifications);
    }

    /// Hands `event` to the dispatcher with whom it is about; nothing while
    /// nobody subscribed.
    fn dispatch(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        part: Option<SecurityAttributes>,
        classifications: &[Classification],
    ) {
        let Some(queue) = self.queue.get() else { return };
        // One event, shared by every subscriber that receives it (issue 503).
        let _ = queue.send(Dispatched::Event(PublishedEvent {
            event: Arc::new(event),
            attributes,
            part,
            classifications: classifications.to_vec(),
        }));
    }
}

#[cfg(test)]
impl EventBus {
    /// Publishes `event` more often than a subscription holds, in a row: a
    /// subscriber that does not run meanwhile falls behind and loses the
    /// oldest, which tests of making up for a loss need (issue 502).
    pub(crate) fn publish_past_capacity(&self, event: &GlobalEvent) {
        for _ in 0..=SUBSCRIPTION_CAPACITY {
            self.publish(event.clone(), SecurityAttributes::systems());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use caelumora_model::actor::ActorId;
    use caelumora_model::channel::{self, ChannelUpdateEvent};
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::group::{Group, GroupId};
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{
        AclEntry, AllowAllPolicy, Memberships, OwnershipPolicy, Secured,
    };
    use caelumora_model::security::{Grant, Subject};
    use caelumora_model::store::Stored;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::value::text::Introduction;
    use caelumora_model::workspace::{Workspace, WorkspaceId};
    use std::slice;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
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
            bus.publish(created(), SecurityAttributes::systems());
        }

        assert!(
            matches!(receiver.recv().await, Err(RecvError::Lagged(5))),
            "the loss reaches the subscriber"
        );
        assert!(receiver.recv().await.is_ok(), "and the newest events are still there");
    }

    #[tokio::test]
    async fn a_subscription_skips_what_it_lost_and_goes_on() {
        let bus = bus();
        let mut subscription = bus.subscribe_all();
        for _ in 0..SUBSCRIPTION_CAPACITY + 5 {
            bus.publish(created(), SecurityAttributes::systems());
        }
        let deleted = SessionId::new();
        bus.publish(GlobalEvent::deleted::<Session>(&deleted), SecurityAttributes::systems());

        subscription.settled().await;
        let mut received = 0;
        while let Some(event) = subscription.try_recv() {
            received += 1;
            if event.about::<Session>() == Some((deleted, &EntityChange::Deleted)) {
                break;
            }
        }
        assert_eq!(
            received, SUBSCRIPTION_CAPACITY,
            "the oldest events are lost, the rest arrives in order"
        );
    }

    /// A subscription whose receiver was dropped is dropped by the bus, the
    /// runtime's as anybody's (issue 504).
    #[tokio::test]
    async fn a_dropped_subscription_is_removed() {
        let bus = bus();
        drop(bus.subscribe_all());
        drop(bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]));
        let kept = bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]);
        bus.publish(created(), SecurityAttributes::systems());
        kept.settled().await;
        assert_eq!(bus.dispatcher.subscribers().len(), 1);
    }

    fn as_user(id: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
    }

    /// What `events` are about: the kind, and the change or the name of a
    /// custom one.
    fn described(events: Vec<GlobalEvent>) -> Vec<String> {
        events
            .iter()
            .map(|event| match &event.change {
                EntityChange::Custom { name, .. } => format!("{} {name}", event.kind),
                EntityChange::Updated(_) => format!("{} Updated", event.kind),
                other => format!("{} {other:?}", event.kind),
            })
            .collect()
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
        bus.publish(
            GlobalEvent::custom::<Channel>(
                &(session, channel),
                channel::JOINED,
                &ActorId::Agent(agent),
            ),
            SecurityAttributes::systems(),
        );
        bus.publish(
            GlobalEvent::custom::<Channel>(
                &(session, channel),
                channel::AGENT_STATE_CHANGED,
                &agent,
            ),
            SecurityAttributes::systems(),
        );

        assert_eq!(
            described(sessions.drain().await),
            ["channel joined", "channel agent_state_changed"],
            "its session"
        );
        assert_eq!(
            described(channels.drain().await),
            ["channel joined", "channel agent_state_changed"],
            "the channel"
        );
        assert!(others.drain().await.is_empty(), "no other channel");
    }

    #[tokio::test]
    async fn a_subscriber_receives_what_it_may_read_and_nothing_else() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);
        let mut carols = bus.subscribe(as_user(carol), vec![SubscriptionTopic::All]);

        let hers = SecurityAttributes::owned_by(ActorId::User(alice))
            .granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let (session, deleted) = (SessionId::new(), SessionId::new());
        bus.publish(GlobalEvent::created::<Session>(&session), hers.clone());
        bus.publish(GlobalEvent::created::<Channel>(&(session, ChannelId::new())), hers.clone());
        bus.publish(GlobalEvent::deleted::<Session>(&deleted), hers.clone());
        bus.publish(
            GlobalEvent::created::<User>(&carol),
            SecurityAttributes::owned_by(ActorId::User(carol)),
        );

        assert_eq!(
            described(alices.drain().await),
            ["session Created", "channel Created", "session Deleted"],
            "the owner gets it all, in order - a deletion too"
        );
        assert_eq!(
            described(bobs.drain().await),
            ["session Created", "channel Created", "session Deleted"],
            "so does a contributor"
        );
        assert_eq!(
            described(carols.drain().await),
            ["user Created"],
            "a stranger only her own user event"
        );
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
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let attributes = SecurityAttributes::owned_by(ActorId::User(alice));
        bus.publish(GlobalEvent::created::<Session>(&session), attributes.clone());
        bus.publish(GlobalEvent::created::<Channel>(&(session, channel)), attributes.clone());
        bus.publish(
            GlobalEvent::custom::<Channel>(
                &(session, channel),
                channel::JOINED,
                &ActorId::User(alice),
            ),
            attributes.clone(),
        );
        bus.publish(
            GlobalEvent::custom::<Channel>(
                &(session, channel),
                channel::AGENT_STATE_CHANGED,
                &AgentId::new(),
            ),
            attributes,
        );

        assert_eq!(
            described(alices.drain().await),
            ["channel Created", "channel joined", "channel agent_state_changed"],
            "asked as reading the channel (issue 304), the session's as reading it"
        );
    }

    #[tokio::test]
    async fn a_deletion_is_received_by_who_may_hear_of_it() {
        use caelumora_model::security::Verb;

        /// Lets hear of deletions alone, so that the question asked shows.
        #[derive(Debug)]
        struct DeletionsOnly;

        #[async_trait::async_trait]
        impl AccessPolicy for DeletionsOnly {
            async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
                matches!(action, ActionType::Entity { verb: Verb::HearDeleted, .. })
            }
        }

        let bus = EventBus::new(Arc::new(DeletionsOnly), Arc::new(ConfidentialityRules::default()));
        let alice = UserId::new();
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let session = SessionId::new();
        let attributes = SecurityAttributes::owned_by(ActorId::User(alice));
        bus.publish(GlobalEvent::created::<Session>(&session), attributes.clone());
        bus.publish(GlobalEvent::deleted::<Session>(&session), attributes.clone());
        bus.publish(GlobalEvent::restored::<Session>(&session), attributes);

        assert_eq!(
            described(alices.drain().await),
            ["session Deleted"],
            "asked as hearing of the deletion (issue 332), the rest as reading"
        );
    }

    #[tokio::test]
    async fn the_events_of_a_classified_channel_reach_who_is_cleared_for_it() {
        use caelumora_model::confidentiality::{Clearance, Compartments, Confidentiality};
        use caelumora_model::role::{RoleId, Roles};
        let staff = RoleId::new();
        let roles = Roles::new([(
            staff,
            "staff".to_string(),
            vec![Clearance { level: Confidentiality::Internal, compartment: None }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        let bus = EventBus::new(Arc::new(OwnershipPolicy::new()), Arc::new(rules));
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut alices = bus
            .subscribe(as_user(alice).with_roles(vec![staff], None), vec![SubscriptionTopic::All]);
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);

        let (session, channel) = (SessionId::new(), ChannelId::new());
        let shared = SecurityAttributes::owned_by(ActorId::User(alice))
            .granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let public = Classification::general(Confidentiality::Public);
        let internal = Classification::general(Confidentiality::Internal);
        bus.publish_classified(
            GlobalEvent::created::<Channel>(&(session, channel)),
            shared.clone(),
            slice::from_ref(&internal),
        );
        bus.publish_classified(
            GlobalEvent::custom::<Channel>(&(session, channel), channel::STARTED, &()),
            shared.clone(),
            slice::from_ref(&public),
        );
        let lowered = ChannelUpdateEvent::ClassificationChanged(public.clone());
        bus.publish_classified(
            GlobalEvent::updated::<Channel>(&(session, channel), &lowered),
            shared,
            &[internal, public],
        );

        assert_eq!(
            described(alices.drain().await),
            ["channel Created", "channel started", "channel Updated"],
            "cleared, she gets it all"
        );
        assert_eq!(described(bobs.drain().await), ["channel started", "channel Updated"], "he reads it, but is cleared only for what is public (issue 312) - and learns of its reclassification");
    }

    #[tokio::test]
    async fn a_profile_s_change_reaches_its_user_alone() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);

        let profile =
            UserProfile::new(alice, Some(Introduction::new("I run the budget.").unwrap()));
        let attributes = Secured::security_attributes(&profile).clone();
        bus.publish(GlobalEvent::updated::<UserProfile>(&alice, &profile), attributes);

        assert_eq!(alices.drain().await.len(), 1, "hers");
        assert!(bobs.drain().await.is_empty(), "not his, although her user record is everybody's");
    }

    /// An event about a part of an entity - a version of an artifact (issue
    /// 316) - reaches whoever may read both the entity and the part.
    #[tokio::test]
    async fn a_part_s_events_reach_whoever_reads_the_entity_and_the_part() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let alice = UserId::new();
        let bob = UserId::new();
        let carol = UserId::new();
        let dave = UserId::new();
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);
        let mut carols = bus.subscribe(as_user(carol), vec![SubscriptionTopic::All]);
        let mut daves = bus.subscribe(as_user(dave), vec![SubscriptionTopic::All]);

        let reads = |user| Subject::Actor(ActorId::User(user));
        let whole = SecurityAttributes::owned_by(ActorId::User(alice))
            .granting(reads(bob), Grant::Read)
            .granting(reads(carol), Grant::Read);
        let part = SecurityAttributes::owned_by(ActorId::User(alice))
            .granting(reads(bob), Grant::Read)
            .granting(reads(dave), Grant::Read);
        bus.publish_classified_part(
            GlobalEvent::created::<Workspace>(&WorkspaceId::new()),
            whole,
            part,
            &[],
        );

        assert_eq!(alices.drain().await.len(), 1, "its owner");
        assert_eq!(bobs.drain().await.len(), 1, "who reads both");
        assert!(carols.drain().await.is_empty(), "not who reads the entity alone");
        assert!(daves.drain().await.is_empty(), "nor who reads the part alone");
    }

    #[tokio::test]
    async fn a_workspace_s_events_reach_whoever_may_use_it() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut alices = bus.subscribe(as_user(alice), vec![SubscriptionTopic::All]);
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);
        let mut carols = bus.subscribe(as_user(carol), vec![SubscriptionTopic::All]);
        let mut everything = bus.subscribe_all();

        let mut workspace = Workspace::new(
            ScopedName::new("docs", Scope::UserScope { user_id: alice }).unwrap(),
            "main",
            Default::default(),
            alice,
        );
        workspace
            .security_attributes
            .acl
            .push(AclEntry { subject: Subject::Actor(ActorId::User(bob)), grant: Grant::Read });
        let attributes = workspace.security_attributes.clone();
        bus.publish(GlobalEvent::created::<Workspace>(&workspace.id), attributes.clone());
        bus.publish(GlobalEvent::deleted::<Workspace>(&workspace.id), attributes);

        assert_eq!(alices.drain().await.len(), 2, "its owner");
        assert_eq!(bobs.drain().await.len(), 2, "a contributor");
        assert!(carols.drain().await.is_empty(), "nobody else");
        assert_eq!(everything.drain().await.len(), 2, "and the runtime, on `All`");
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_reaches_every_subscriber() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let mut someone = bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]);
        bus.publish(
            GlobalEvent::created::<Session>(&SessionId::new()),
            SecurityAttributes::systems(),
        );
        assert_eq!(someone.drain().await.len(), 1);
    }

    /// Issue 503: an event is decided for every subscriber at once, so a group
    /// its attributes name is read once, however many streams are open.
    #[tokio::test]
    async fn a_group_an_event_names_is_read_once_for_every_subscriber() {
        /// Every group has `members`; counts how often one is asked for.
        #[derive(Debug, Default)]
        struct Counting {
            members: Vec<ActorId>,
            asked: AtomicUsize,
        }
        #[async_trait]
        impl Memberships for Counting {
            async fn group(&self, id: GroupId) -> Option<Group> {
                self.asked.fetch_add(1, Ordering::SeqCst);
                let name = ScopedName::new("place", Scope::GlobalScope).ok()?;
                Some(Group {
                    id,
                    members: self.members.clone(),
                    ..Group::team(name, UserId::new())
                })
            }
        }
        let (alice, bob) = (UserId::new(), UserId::new());
        let counting =
            Arc::new(Counting { members: vec![ActorId::User(bob)], ..Default::default() });
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::with_memberships(Arc::<Counting>::clone(&counting))),
            Arc::new(ConfidentialityRules::default()),
        );
        let mut bobs = bus.subscribe(as_user(bob), vec![SubscriptionTopic::All]);
        let mut strangers: Vec<EventSubscription> = (0..5)
            .map(|_| bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]))
            .collect();
        let (session, cid) = (SessionId::new(), ChannelId::new());

        bus.publish(
            GlobalEvent::custom::<Channel>(&(session, cid), channel::JOINED, &ActorId::User(bob)),
            Channel::attributes_of(cid, ActorId::User(alice)),
        );

        assert_eq!(described(bobs.drain().await), ["channel joined"], "a member hears of it");
        for stranger in &mut strangers {
            assert!(stranger.drain().await.is_empty(), "who is no member does not");
        }
        assert_eq!(
            counting.asked.load(Ordering::SeqCst),
            1,
            "its group read once for six subscribers"
        );
    }

    /// Issue 503: an event is made once and shared by every subscriber that
    /// receives it - the runtime's as anybody's -, not copied for each.
    #[tokio::test]
    async fn an_event_is_shared_by_its_subscribers_not_copied() {
        let bus = bus();
        let mut first = bus.subscribe_all().into_receiver();
        let mut second = bus.subscribe_all().into_receiver();
        let mut someones =
            bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]).into_receiver();

        bus.publish(created(), SecurityAttributes::systems());

        let one = first.recv().await.unwrap();
        let other = second.recv().await.unwrap();
        let reached =
            time::timeout(Duration::from_secs(2), someones.recv()).await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&one, &other), "one event for the runtime's subscribers");
        assert!(Arc::ptr_eq(&one, &reached), "and for those in someone's name");
    }

    /// Issue 504: the runtime's subscriptions and those in someone's name
    /// are served on one path and receive what they share alike, in the
    /// order it was published - as each path kept its own order before.
    #[tokio::test]
    async fn every_subscriber_receives_the_events_in_the_order_they_were_published() {
        let bus = bus();
        let mut runtime = bus.subscribe_all();
        let mut someone = bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]);
        let (session, cid) = (SessionId::new(), ChannelId::new());
        let agent = AgentId::new();

        bus.publish(GlobalEvent::created::<Session>(&session), SecurityAttributes::systems());
        bus.publish(
            GlobalEvent::custom::<Channel>(
                &(session, cid),
                channel::JOINED,
                &ActorId::Agent(agent),
            ),
            SecurityAttributes::systems(),
        );
        bus.publish(GlobalEvent::deleted::<Session>(&session), SecurityAttributes::systems());

        let expected = ["session Created", "channel joined", "session Deleted"];
        assert_eq!(described(runtime.drain().await), expected, "the runtime's");
        assert_eq!(described(someone.drain().await), expected, "and someone's, alike");
    }

    /// Issue 504: a subscription in someone's name follows a topic as the
    /// runtime's do - what it is about, and still only what it may read.
    #[tokio::test]
    async fn a_subscription_in_someones_name_follows_a_topic() {
        let bus = EventBus::new(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        );
        let alice = UserId::new();
        let (theirs, other) = (AgentId::new(), AgentId::new());
        let mut hers = bus.subscribe(as_user(alice), vec![SubscriptionTopic::Agent(other)]);
        let readable = SecurityAttributes::owned_by(ActorId::User(alice));
        let unreadable = SecurityAttributes::owned_by(ActorId::User(UserId::new()));

        bus.publish(GlobalEvent::created::<Agent>(&theirs), readable.clone());
        assert!(hers.drain().await.is_empty(), "not of a topic it does not follow");

        bus.follow(&hers, SubscriptionTopic::Agent(theirs));
        bus.publish(GlobalEvent::deleted::<Agent>(&theirs), readable);
        bus.publish(GlobalEvent::restored::<Agent>(&theirs), unreadable);
        assert_eq!(
            described(hers.drain().await),
            ["agent Deleted"],
            "once it follows it, what it may read"
        );
    }

    /// Issue 504: the runtime's subscriptions, made as the system, are not
    /// asked about - a policy that refuses everybody keeps nothing from
    /// them, and everything from a subscription in someone's name.
    #[tokio::test]
    async fn the_runtimes_subscriptions_are_not_asked_about() {
        /// Refuses every question, the system's too.
        #[derive(Debug)]
        struct RefusesAll;
        #[async_trait]
        impl AccessPolicy for RefusesAll {
            async fn check(&self, _: &SecurityContext, _: ActionType<'_>) -> bool {
                false
            }
        }
        let bus = EventBus::new(Arc::new(RefusesAll), Arc::new(ConfidentialityRules::default()));
        let mut runtime = bus.subscribe_all();
        let mut someone = bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]);

        bus.publish(created(), SecurityAttributes::systems());

        assert_eq!(described(runtime.drain().await), ["session Created"], "the runtime's");
        assert!(someone.drain().await.is_empty(), "not someone's");
    }

    /// A subscription kept whole learns once the bus is gone, as one whose
    /// receiver was taken.
    #[tokio::test]
    async fn a_subscription_learns_once_the_bus_is_gone() {
        let bus = bus();
        let mut runtime = bus.subscribe_all();
        let mut someone = bus.subscribe(as_user(UserId::new()), vec![SubscriptionTopic::All]);

        drop(bus);

        let closed = time::timeout(Duration::from_secs(2), async {
            (runtime.recv().await, someone.recv().await)
        });
        assert!(matches!(closed.await, Ok((None, None))), "both are closed");
    }
}
