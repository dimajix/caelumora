//! What tests of the runtime need to build it as a binary does, and the fixtures the tests of
//! sessions, channels and the views share: one of each, for building a runtime, a user, a session
//! and its main channel, and the views an actor of a running session navigates (issue 315).
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use crate::actor::user::ChannelWatch;
use crate::channel::ChannelLoop;
use crate::event::EventSubscription;
use crate::runtime::{ActorRuntime, ChannelRuntime, RuntimeManager};
use crate::session::{ChannelSpec, SessionLoop, SessionManager};
pub use crate::testing::actor_runtime_in;
use crate::testing::TestRuntime;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::actor::{ActorEvent, Rejection};
use caelumora_model::channel::testing::make_channel;
use caelumora_model::channel::{Channel, ChannelState};
use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use caelumora_model::event::GlobalEvent;
use caelumora_model::history::{Audience, HistoryEntry, Recorded};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::project::ProjectId;
use caelumora_model::security::SecurityContext;
use caelumora_model::security::{AccessPolicy, Admission, AllowAllPolicy, OwnershipPolicy};
use caelumora_model::session::Participant;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::query::{FieldPath, Filter};
use caelumora_model::store::transcript::TranscriptStore;
use caelumora_model::store::Persistence;
use caelumora_model::store::{
    EntityKind, PersistenceBackend, Presence, StateStore, StoreError, StoreResult,
};
use caelumora_model::user::{User, UserId};
use caelumora_model::value::name::Name;
use caelumora_model::value::name::ScopedName;
use chrono::Utc;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use uuid::Uuid;

/// A user named `name`, not stored.
pub fn user(name: &str) -> User {
    User {
        id: UserId::new(),
        username: Name::new(name).unwrap(),
        email: String::new(),
        roles: Vec::new(),
        deleted: None,
    }
}

/// `users` as participants, recorded in the store the session reads
/// them from (issue 120).
pub async fn record_users(manager: &SessionManager, users: &[&User]) -> Vec<Participant> {
    let store = &manager.runtime().entity_stores().users;
    let mut participants = Vec::new();
    for user in users {
        store.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    participants
}

/// A runtime under the ownership policy, which learns who belongs to a
/// group from its store (issue 23), with `rules` as the confidentiality
/// policy beside it (issue 304), and a manager on it.
pub fn owned_runtime(rules: ConfidentialityRules) -> (Persistence, Arc<SessionManager>) {
    let memberships = Arc::new(StoreMemberships::new());
    let access_policy: Arc<dyn AccessPolicy> =
        Arc::new(OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let confidentiality: Arc<dyn ConfidentialityPolicy> = Arc::new(rules);
    let store =
        Persistence::in_memory_with(Arc::clone(&access_policy), Arc::clone(&confidentiality));
    memberships.bind(&store.entity_stores().unwrap().groups);
    let runtime =
        TestRuntime { store: store.clone(), access_policy, confidentiality, ..Default::default() }
            .build();
    (store, Arc::new(SessionManager::new(runtime)))
}

/// A runtime under no policy: everything is allowed (`TestRuntime`).
pub fn test_runtime() -> Arc<RuntimeManager> {
    TestRuntime::default().build()
}

/// The runtime of an actor in a channel of no running session, never
/// attached - for turns and what an actor is built with in tests that reach
/// no channel: its views read nothing elsewhere, its questions have nobody
/// to be put to, and it has no workspace.
pub fn detached_actor_runtime() -> ActorRuntime {
    let session = bare_session(SessionId::new());
    let channel = bare_channel(session.id);
    detached_actor_runtime_in(test_runtime(), session, channel)
}

/// As `detached_actor_runtime`, on `runtime_manager`, in `channel` of
/// `session` - for tests that give the actor the policy, the rules, the
/// session and the channel of their own (issues 371, 374).
pub fn detached_actor_runtime_in(
    runtime_manager: Arc<RuntimeManager>,
    session: Session,
    channel: Channel,
) -> ActorRuntime {
    ChannelRuntime::outside_a_session(runtime_manager, session, channel)
        .new_actor_runtime(ActorId::None)
}

/// A session that holds nothing but its id `session_id`: what a channel
/// outside a running session belongs to (`ChannelRuntime::outside_a_session`).
pub fn bare_session(session_id: SessionId) -> Session {
    let now = Utc::now();
    Session {
        summary: None,
        briefing: None,
        language: None,
        time_zone: None,
        id: session_id,
        name: Name::new("s").unwrap(),
        participants: Vec::new(),
        channels: Vec::new(),
        commands: Default::default(),
        privacy: Default::default(),
        approval_rules: Vec::new(),
        workspace: None,
        project: ProjectId::new(),
        created_at: now,
        updated_at: now,
        security_attributes: Default::default(),
        deleted: None,
    }
}

/// A channel of the session `session_id` that holds nothing but its ids:
/// what an actor outside a running session is in
/// (`ChannelRuntime::outside_a_session`).
pub fn bare_channel(session_id: SessionId) -> Channel {
    Channel {
        summary: None,
        briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: false,
        id: ChannelId::new(),
        session_id,
        name: Name::new("c").unwrap(),
        components: Default::default(),
        classification: Default::default(),
        language: None,
        participants: Vec::new(),
        internal_actors: Vec::new(),
        security_attributes: Default::default(),
        deleted: None,
        former: Vec::new(),
    }
}

/// A session of `owner` named `name`, as a template builds one (`SessionTemplate::build`): read by
/// its members (`Session::attributes_of`), the group of its participants (issue 23), whom the test
/// sets. It lists one channel, `main_channel`. Its project is a stand-in: the runtime makes it one
/// of its own when it stores it (issue 467).
pub fn make_session(name: &str, owner: UserId) -> Session {
    let id = SessionId::new();
    let now = Utc::now();
    Session {
        summary: None,
        briefing: None,
        language: None,
        time_zone: None,
        approval_rules: Vec::new(),
        workspace: None,
        project: ProjectId::new(),
        commands: Default::default(),
        privacy: Default::default(),
        id,
        name: Name::new(name).unwrap(),
        participants: Vec::new(),
        channels: vec![ChannelId::new()],
        created_at: now,
        updated_at: now,
        security_attributes: Session::attributes_of(id, owner, &[]),
        deleted: None,
    }
}

/// The state of `channel` - a channel a test stores as it is, its
/// participants written into its document - with their joins recorded in its
/// history, in its owner's name, as if they had founded it (issue 420): so
/// that its history says who is a member, as it does of a channel created
/// by the one way in. What a test records in the history goes after them.
pub fn founded(channel: &Channel) -> ChannelState {
    let mut state = ChannelState::new(channel.id);
    let owner = channel.security_attributes.owner_id;
    for participant in &channel.participants {
        let actor = participant.actor_id();
        let joined = HistoryEntry::ParticipantJoined {
            participant: actor,
            name: actor.to_string(),
            by: owner,
            by_name: owner.to_string(),
            at: Utc::now(),
        };
        state.history.enter(Recorded::new(joined, owner, Audience::Everybody));
    }
    state
}

/// The channel `session` lists, as a document of its own (issue 70) and as
/// `Channel::build` makes one: "main", belonging to whom the session belongs
/// to, read by its participants - the session's participants.
pub fn main_channel(session: &Session) -> Channel {
    let id = session.channels[0];
    // The id the session lists, and the attributes that go with it.
    Channel {
        id,
        security_attributes: Channel::attributes_of(id, session.security_attributes.owner_id),
        participants: session.participants.clone(),
        ..make_channel(session.id, "main")
    }
}

/// Creates and starts `session` with its main channel.
pub async fn create(
    manager: &SessionManager,
    ctx: &SecurityContext,
    session: Session,
) -> Result<Arc<SessionLoop>> {
    let channels = vec![main_channel(&session)];
    manager.create(ctx, session, channels).await
}

/// Stores `session` with `channels` on behalf of `ctx` as the runtime stores a brand-new one
/// (`SessionManager::persist`) - with a project of its own -, without announcing or starting it:
/// the session as stored. The channels keep the participants they name, without their joins in
/// their histories (`founded`).
pub async fn persist(
    manager: &SessionManager,
    ctx: &SecurityContext,
    session: Session,
    channels: &[Channel],
) -> Session {
    let own = session.new_own_project().expect("a session of a user");
    let session = Session { project: own.id, ..session };
    manager.persist(ctx, &session, &own, channels).await.expect("the session is stored");
    session
}

/// What tests of other crates read of a session manager past what its users may (issue 486):
/// behind the feature `test-support` alone, under a name of its own - the manager's API is not
/// widened by it.
impl SessionManager {
    /// The access policy of the runtime it starts its sessions in
    /// (`RuntimeManager::access_policy`): what a test builds a component with, as the runtime gives
    /// a component its policy.
    pub fn runtime_access_policy(&self) -> &Arc<dyn AccessPolicy> {
        self.runtime().access_policy()
    }
}

/// Drain every event currently buffered on the receiver.
pub fn drain(rx: &mut EventSubscription) -> Vec<GlobalEvent> {
    let mut out = Vec::new();
    while let Some(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// A running session owned by a user who takes part in it, with that
/// user's context and a second user of the session.
pub async fn running_session_of_a_user(
    manager: &SessionManager,
) -> (Arc<SessionLoop>, SecurityContext, ActorId) {
    let (owner, other) = (user("owner"), user("other"));
    let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
    let mut session = make_session("with-users", owner.id);
    session.participants = record_users(manager, &[&owner, &other]).await;
    let ctx = SecurityContext::new(owner_id, session.id, Default::default());
    let session_loop = create(manager, &ctx, session).await.expect("the session starts");
    (session_loop, ctx, other_id)
}

pub fn spec(name: &str, participants: Vec<ActorId>) -> ChannelSpec {
    ChannelSpec {
        name: name.to_string(),
        components: Default::default(),
        participants,
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    }
}

/// The message and the reason of the next `MessageRejected` `events`
/// show: why the channel did not let in what their user sent (issue 304).
pub async fn rejection(events: &mut ChannelWatch) -> (Option<MessageId>, Rejection) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            if let ActorEvent::MessageRejected { message, reason, .. } =
                events.recv().await.expect("the channel is running").event
            {
                return (message, reason);
            }
        }
    })
    .await
    .expect("the channel tells why it did not let it in")
}

pub async fn history_of(channel: &ChannelLoop, count: usize) -> Vec<Message> {
    for _ in 0..100 {
        let history = channel.history().await;
        if history.len() >= count {
            return history;
        }
        time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the channel's history never reached {count} messages");
}

/// A runtime whose entities are kept in memory, and whose documents of `kind` fail to be replaced -
/// marked deleted, restored or changed - once `passing` is set to how many may pass first: counted
/// down past zero, it never fails again. For tests of what an operation sets back when a step of
/// it cannot be stored.
pub fn failing_runtime(kind: EntityKind) -> (Arc<RuntimeManager>, Arc<AtomicUsize>) {
    let passing = Arc::new(AtomicUsize::new(usize::MAX));
    let entities =
        Arc::new(FailingBackend { inner: MemoryStore::new(), kind, passing: Arc::clone(&passing) });
    let store = Persistence::new(
        entities,
        Arc::new(MemoryStore::new()),
        Arc::new(AllowAllPolicy::new()),
        Arc::new(ConfidentialityRules::default()),
    )
    .unwrap();
    (TestRuntime { store, ..Default::default() }.build(), passing)
}

/// A backend of the entities in memory whose documents of `kind` are not
/// replaced - marked deleted, restored or changed - once, after as many
/// as `passing` let through: counted down past zero, never again.
#[derive(Debug)]
struct FailingBackend {
    inner: MemoryStore,
    kind: EntityKind,
    passing: Arc<AtomicUsize>,
}

impl PersistenceBackend for FailingBackend {
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        let documents = self.inner.documents(kind, document_of)?;
        if kind != self.kind {
            return Ok(documents);
        }
        Ok(Arc::new(FailingDocuments { inner: documents, passing: Arc::clone(&self.passing) }))
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Err(StoreError::Backend("the entities alone".to_string()))
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Err(StoreError::Backend("the entities alone".to_string()))
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Err(StoreError::Backend("the entities alone".to_string()))
    }
}

#[derive(Debug)]
struct FailingDocuments {
    inner: Arc<dyn DocumentStorage>,
    passing: Arc<AtomicUsize>,
}

#[async_trait]
impl DocumentStorage for FailingDocuments {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        self.inner.get(id).await
    }
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        admission: Option<&Admission>,
        presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        self.inner.scan(filter, admission, presence, after, limit).await
    }
    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        self.inner.named(name).await
    }
    async fn insert(&self, document: Document) -> StoreResult<()> {
        self.inner.insert(document).await
    }
    async fn replace(&self, document: Document) -> StoreResult<()> {
        if self.passing.fetch_sub(1, Ordering::SeqCst) == 0 {
            return Err(StoreError::Backend("failing".to_string()));
        }
        self.inner.replace(document).await
    }
    async fn remove_deleted(&self, id: Uuid) -> StoreResult<bool> {
        self.inner.remove_deleted(id).await
    }
}
