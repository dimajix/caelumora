//! What tests of the kernel need to build it as a binary does, and the fixtures the tests of
//! sessions, channels and the views share: one of each, for building a kernel, recording users,
//! storing and starting a session, and the views an actor of a running session navigates (issue
//! 315). The documents of a session, its channels and its users are the domain's
//! (`caelumora_domain::session::testing`, `channel::testing`, `user::testing`).
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

pub use crate::testing::participant_runtime_in;

use crate::actor::user::ChannelWatch;
use crate::channel::ChannelLoop;
use crate::kernel::{ChannelRuntime, Kernel, ParticipantRuntime};
use crate::session::{ChannelSpec, SessionLoop, SessionManager};
use crate::testing::TestKernel;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::actor::{ActorEvent, Rejection};
use caelumora_domain::channel::testing::main_channel;
use caelumora_domain::channel::Channel;
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::session::testing::make_session;
use caelumora_domain::session::Session;
use caelumora_domain::store::query::{FieldPath, Filter};
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::user::testing::make_user;
use caelumora_domain::value::name::ScopedName;
use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use caelumora_security::SecurityContext;
use caelumora_security::{AccessPolicy, Admission, OwnershipPolicy};
use caelumora_store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_store::history::HistoryStore;
use caelumora_store::memberships::StoreMemberships;
use caelumora_store::memory::MemoryStore;
use caelumora_store::testing::record_users;
use caelumora_store::transcript::TranscriptStore;
use caelumora_store::Persistence;
use caelumora_store::{PersistenceBackend, Presence, StateStore};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use uuid::Uuid;

/// A kernel under the ownership policy, which learns who belongs to a group from its store (issue
/// 23), with `rules` as the confidentiality policy beside it (issue 304), and a manager on it -
/// what a test writes, it writes through the kernel's stores, which check as the kernel does.
pub fn owned_kernel(rules: ConfidentialityRules) -> (Arc<Kernel>, Arc<SessionManager>) {
    owned_kernel_under(Arc::new(rules))
}

/// As `owned_kernel`, with `confidentiality` as the confidentiality policy: a test's own, for what
/// the kernel decides by it (issue 516).
pub fn owned_kernel_under(
    confidentiality: Arc<dyn ConfidentialityPolicy>,
) -> (Arc<Kernel>, Arc<SessionManager>) {
    let memberships = Arc::new(StoreMemberships::new());
    let access_policy: Arc<dyn AccessPolicy> =
        Arc::new(OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let kernel = TestKernel { access_policy, confidentiality, ..Default::default() }.build();
    memberships.bind(&kernel.entity_stores().groups);
    (Arc::clone(&kernel), Arc::new(SessionManager::new(kernel)))
}

/// A kernel under no policy: everything is allowed (`TestKernel`).
pub fn test_kernel() -> Arc<Kernel> {
    TestKernel::default().build()
}

/// The runtime of the participant `actor` on `kernel`, in `channel` of `session`, which
/// runs in no session and is never attached - for tests that give the actor the policy, the
/// rules, the session and the channel of their own (issues 371, 374), acting in its own name
/// without roles (issue 516): its views read nothing elsewhere, its questions have nobody to be
/// put to, and it has no workspace.
pub fn detached_participant_in(
    kernel: Arc<Kernel>,
    session: Session,
    channel: Channel,
    actor: ActorId,
) -> ParticipantRuntime {
    ChannelRuntime::outside_a_session(kernel, session, channel).new_participant_runtime(actor)
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

/// Stores `session` with `channels` on behalf of `ctx` as the kernel stores a brand-new one
/// (`SessionManager::persist`) - with a project of its own -, without announcing or starting it:
/// the session as stored, and `channels` then as stored, too. The channels keep the participants
/// they name, without their joins in their histories (the domain's `channel::testing::founded`).
pub async fn persist(
    manager: &SessionManager,
    ctx: &SecurityContext,
    session: Session,
    channels: &mut [Channel],
) -> Session {
    let mut own = session.new_own_project().expect("a session of a user");
    let mut session = Session { project: own.id, ..session };
    manager.persist(ctx, &mut session, &mut own, channels).await.expect("the session is stored");
    session
}

/// What tests of other crates read of a session manager past what its users may (issue 486):
/// behind the feature `test-support` alone, under a name of its own - the manager's API is not
/// widened by it.
impl SessionManager {
    /// The access policy of the kernel it starts its sessions in (`Kernel::access_policy`): what a
    /// test builds a component with, as the kernel gives a component its policy.
    pub fn kernel_access_policy(&self) -> &Arc<dyn AccessPolicy> {
        self.kernel().access_policy()
    }
}

/// A running session owned by a user who takes part in it, with that
/// user's context and a second user of the session.
pub async fn running_session_of_a_user(
    manager: &SessionManager,
) -> (Arc<SessionLoop>, SecurityContext, ActorId) {
    let (owner, other) = (make_user("owner"), make_user("other"));
    let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
    let mut session = make_session("with-users", owner.id);
    session.participants = record_users(manager.kernel().entity_stores(), &[&owner, &other]).await;
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

/// A kernel whose entities are kept in memory, and whose documents of `kind` fail to be replaced -
/// marked deleted, restored or changed - once `passing` is set to how many may pass first: counted
/// down past zero, it never fails again. For tests of what an operation sets back when a step of it
/// cannot be stored.
pub fn failing_kernel(kind: EntityKind) -> (Arc<Kernel>, Arc<AtomicUsize>) {
    let passing = Arc::new(AtomicUsize::new(usize::MAX));
    let entities =
        Arc::new(FailingBackend { inner: MemoryStore::new(), kind, passing: Arc::clone(&passing) });
    let store = Persistence::new(entities, Arc::new(MemoryStore::new())).unwrap();
    (TestKernel { store, ..Default::default() }.build(), passing)
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
    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        if self.passing.fetch_sub(1, Ordering::SeqCst) == 0 {
            return Err(StoreError::Backend("failing".to_string()));
        }
        self.inner.replace(document, read).await
    }
    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        self.inner.remove(id, read).await
    }
}
