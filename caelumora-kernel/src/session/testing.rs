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
use crate::actor::Shown;
use crate::channel::ChannelLoop;
use crate::kernel::{ChannelRuntime, Kernel, ParticipantRuntime};
use crate::procedure::participant::{
    take_in, Entry, LetGo, LetGoFromThread, TakeIn, TakeIntoThread,
};
use crate::procedure::session::{
    establish, persist as persist_new, record_creation, resume_interrupted, shut_down, Create,
    Delete, Move, Open, Purge, Restore, Stop,
};
use crate::procedure::{channel, thread};
use crate::procedure::{Compensation, Deferred, Key, Locks, Procedure};
use crate::session::{ChannelSpec, SessionError, SessionLoop, SessionManager, SessionSpec};
use crate::testing::TestKernel;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::actor::{ActorEvent, Rejection};
use caelumora_domain::channel::testing::main_channel;
use caelumora_domain::channel::{
    Channel, ChannelId, Circle, HistoryAccess, Participant, ThreadId, ThreadOrigin,
};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::{FieldPath, Filter};
use caelumora_domain::scope::Scope;
use caelumora_domain::session::testing::make_session;
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::Stored;
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::template::SessionTemplateId;
use caelumora_domain::user::testing::make_user;
use caelumora_domain::value::name::ScopedName;
use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use caelumora_security::{AccessPolicy, Admission, OwnershipPolicy};
use caelumora_security::{ActionType, Enforcement, SecurityContext};
use caelumora_store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_store::history::HistoryStore;
use caelumora_store::memberships::StoreMemberships;
use caelumora_store::memory::MemoryStore;
use caelumora_store::places::StorePlaces;
use caelumora_store::sessions::{SessionHold, SessionLocks};
use caelumora_store::testing::record_users;
use caelumora_store::transcript::TranscriptStore;
use caelumora_store::Persistence;
use caelumora_store::{PersistenceBackend, Presence, StateStore};
use serde_json::Value;
use std::collections::HashSet;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    owned(TestKernel { confidentiality, ..Default::default() })
}

/// `kernel` built under the ownership policy, which learns who belongs to a group from its store,
/// and a manager on it, as `owned_kernel` builds them: for a test that names more of its kernel -
/// the plugins and the libraries it runs with (issue 518).
pub fn owned(kernel: TestKernel<'_>) -> (Arc<Kernel>, Arc<SessionManager>) {
    let memberships = Arc::new(StoreMemberships::new());
    let access_policy: Arc<dyn AccessPolicy> =
        Arc::new(OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let kernel = TestKernel { access_policy, ..kernel }.build();
    memberships.bind(&kernel.entity_stores().groups);
    (Arc::clone(&kernel), SessionManager::new(kernel))
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
    manager: &Arc<SessionManager>,
    ctx: &SecurityContext,
    session: Session,
) -> Result<Arc<SessionLoop>> {
    let channels = vec![main_channel(&session)];
    manager.create(ctx, session, channels).await
}

/// Stores `session` with `channels` on behalf of `ctx` as the kernel stores a brand-new one
/// (`procedure::session::persist`) - with a project of its own, each announced as it is made -,
/// without starting it: the session as stored, and `channels` then as stored, too. The channels
/// keep the participants they name, without their joins in their histories (the domain's
/// `channel::testing::founded`).
pub async fn persist(
    manager: &SessionManager,
    ctx: &SecurityContext,
    session: Session,
    channels: &mut [Channel],
) -> Session {
    let mut own = session.new_own_project().expect("a session of a user");
    let mut session = Session { project: own.id, ..session };
    let kernel = manager.kernel();
    let storing = persist_new(kernel, ctx, &mut session, &mut own, channels);
    storing.await.expect("the session is stored");
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

/// What the tests do with sessions as the entry points do (issue 593): each a procedure, carried
/// out by the kernel's executor (`Kernel::procedures`), under the verb the entry points ask for -
/// the manager itself offers only the register of the running sessions and the lifecycles -,
/// failing as the procedures fail (`SessionError::untyped`), so that a test looks into the error as
/// it did before.
impl SessionManager {
    /// Creates `session` with its `channels` on behalf of `context`, as a template would
    /// (`Built`).
    ///
    /// # Errors
    /// As `procedure::session::establish` and taking the founders in.
    pub async fn create(
        self: &Arc<Self>,
        context: &SecurityContext,
        session: Session,
        channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        let creating =
            Built { sessions: Arc::clone(self), context: context.clone(), session, channels };
        self.kernel().procedures().run(creating).await.map_err(SessionError::untyped)
    }

    /// Creates a session from the template `template_id` as `spec` asks, on behalf of `context`
    /// (`procedure::session::Create::from_template`).
    ///
    /// # Errors
    /// As `procedure::session::Create`.
    pub async fn create_from_template(
        &self,
        context: &SecurityContext,
        template_id: SessionTemplateId,
        spec: SessionSpec,
    ) -> Result<Arc<SessionLoop>> {
        let creating = Create::from_template(context, template_id, spec);
        self.kernel().procedures().run(creating).await.map_err(SessionError::untyped)
    }

    /// Opens session `id` for `context` (`procedure::session::Open`), and builds again the
    /// channels left out as it started (`procedure::channel::Start`), as the REST server does.
    ///
    /// # Errors
    /// As `Open`.
    pub async fn open(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        let procedures = self.kernel().procedures();
        let opened = procedures.run(Open::new(context, id)).await;
        let opened = opened.map_err(SessionError::untyped)?;
        // Its channels left out as it started built again, as the REST server has them (issue 463).
        let starting = channel::Start::left_out(id);
        procedures.run(starting).await.map_err(SessionError::untyped)?;
        Ok(opened)
    }

    /// Stops session `id` on behalf of `context` (`procedure::session::Stop`).
    ///
    /// # Errors
    /// As `Stop`.
    pub async fn stop(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        self.kernel().procedures().run(Stop::new(context, id)).await.map_err(SessionError::untyped)
    }

    /// Deletes session `id` on behalf of `context` (`procedure::session::Delete`).
    ///
    /// # Errors
    /// As `procedure::session::delete`.
    pub async fn delete(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        self.kernel()
            .procedures()
            .run(Delete::new(context, id))
            .await
            .map_err(SessionError::untyped)
    }

    /// Restores session `id` on behalf of `context` (`procedure::session::Restore`).
    ///
    /// # Errors
    /// As `procedure::session::restore`.
    pub async fn restore(&self, context: &SecurityContext, id: SessionId) -> Result<Session> {
        self.kernel()
            .procedures()
            .run(Restore::new(context, id))
            .await
            .map_err(SessionError::untyped)
    }

    /// Purges session `id` on behalf of `context` (`procedure::session::Purge`).
    ///
    /// # Errors
    /// As `procedure::session::purge`.
    pub async fn purge(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        self.kernel().procedures().run(Purge::new(context, id)).await.map_err(SessionError::untyped)
    }

    /// Puts session `id` into `project` on behalf of `context`, or into its own with `None`
    /// (`procedure::session::Move`).
    ///
    /// # Errors
    /// As `Move`.
    pub async fn update_project(
        &self,
        context: &SecurityContext,
        id: SessionId,
        project: Option<ProjectId>,
    ) -> Result<()> {
        self.kernel()
            .procedures()
            .run(Move::new(context, id, project))
            .await
            .map_err(SessionError::untyped)
    }

    /// Takes `participant` into session `id` by their own act, as `TakeIn::joining` does.
    ///
    /// # Errors
    /// As `TakeIn::joining`.
    pub async fn join(
        &self,
        context: &SecurityContext,
        id: SessionId,
        participant: ActorId,
        into: Option<ChannelId>,
    ) -> Result<Arc<SessionLoop>> {
        let joining = TakeIn::joining(context, id, participant, into);
        self.kernel().procedures().run(joining).await.map_err(SessionError::untyped)
    }

    /// Shuts the sessions down as the process ends (`procedure::session::shut_down`).
    pub async fn shutdown(&self) {
        shut_down(self.kernel().procedures()).await;
    }

    /// Starts the sessions interrupted again (`procedure::session::resume_interrupted`).
    pub async fn resume_interrupted(&self) -> usize {
        resume_interrupted(self.kernel()).await
    }
}

/// Creates `session` with its `channels` on behalf of `context`, as a template would build them,
/// and starts it - holding it, and rolling its creation back if it fails, as
/// `procedure::session::Create` does: asked to create it (`CreateSession`) before anything else,
/// its results going to a project of its own, made with it, whatever project it names (issue
/// 467), and the participants the channels name joining them as their founders once it runs, as a
/// template's do (issue 420).
struct Built {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: Session,
    channels: Vec<Channel>,
}

#[async_trait]
impl Procedure for Built {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.session.id))
    }

    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { sessions, context, mut session, mut channels } = self;
        record_creation(log, sessions.kernel(), session.id);
        let creating = ActionType::create(Session::KIND, &session.security_attributes);
        kernel.access_policy().require(&context, creating, StoreError::PermissionDenied).await?;
        let owner = session.security_attributes.owner_id;
        let own = session.new_own_project().ok_or(SessionError::OwnerNotAUser(owner))?;
        session.project = own.id;
        let founders: Vec<(ChannelId, Vec<Participant>)> = channels
            .iter_mut()
            .map(|channel| (channel.id, mem::take(&mut channel.participants)))
            .collect();
        let session_loop = establish(kernel, &context, session, own, channels).await?;
        for (channel, participants) in founders {
            let channel = session_loop
                .channel(channel)
                .ok_or(SessionError::ChannelNotFound { session: session_loop.id(), channel })?;
            let into = Some((&channel, HistoryAccess::None, ThreadId::line(channel.id())));
            for participant in participants {
                let founder = participant.actor_id();
                take_in(log, kernel, &session_loop, &context, founder, Entry::Founding, into)
                    .await?;
            }
        }
        Ok(session_loop)
    }
}

/// A running session owned by a user who takes part in it, with that
/// user's context and a second user of the session.
pub async fn running_session_of_a_user(
    manager: &Arc<SessionManager>,
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
    ChannelSpec { participants, ..ChannelSpec::new(name) }
}

/// The message and the reason of the next `EventRejected` `events` show: why the channel did not
/// let in what their user sent (issue 304).
pub async fn rejection(events: &mut ChannelWatch) -> (Option<MessageId>, Rejection) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            if let ActorEvent::EventRejected { message, reason, .. } =
                events.recv().await.expect("the channel is running").event
            {
                return (message, reason);
            }
        }
    })
    .await
    .expect("the channel tells why it did not let it in")
}

/// Whether `watch` shows within `within` that a run of its user's actor ended (`Shown::Ended`,
/// issue 386).
pub async fn ended(watch: &mut ChannelWatch, within: Duration) -> bool {
    time::timeout(within, async { while !matches!(watch.next().await, Ok(Shown::Ended)) {} })
        .await
        .is_ok()
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

/// A kernel whose stores another process shares (issue 554), as `shared_store` makes them.
pub fn shared_kernel() -> (Arc<Kernel>, Arc<SharedLocks>) {
    let (store, locks) = shared_store();
    (TestKernel { store, ..Default::default() }.build(), locks)
}

/// Stores in memory that behave as the home store does (`HomeLikeBackend`): another process
/// shares them (issue 554), their holds of sessions `SharedLocks` - what tests tell them the other
/// holds, and what they held and let go of -, and they tell where they keep what lies in a scope
/// (issue 577).
pub fn shared_store() -> (Persistence, Arc<SharedLocks>) {
    let locks = Arc::new(SharedLocks::default());
    let entities =
        Arc::new(HomeLikeBackend { inner: MemoryStore::new(), locks: Arc::clone(&locks) });
    (Persistence::new(entities, Arc::new(MemoryStore::new())).unwrap(), locks)
}

/// The holds of sessions of a `shared_kernel`, as the lock of a file would keep them.
#[derive(Debug, Default)]
pub struct SharedLocks {
    /// The sessions the other process holds.
    pub elsewhere: Mutex<HashSet<SessionId>>,
    /// The sessions held here, until their holds are dropped.
    here: Arc<Mutex<HashSet<SessionId>>>,
    /// The sessions whose holds were forgotten, as they were purged.
    pub forgotten: Mutex<Vec<SessionId>>,
}

impl SharedLocks {
    /// Whether session `id` is held here.
    pub fn holds(&self, id: SessionId) -> bool {
        self.here.lock().unwrap().contains(&id)
    }
}

impl SessionLocks for SharedLocks {
    fn try_hold(&self, id: SessionId) -> StoreResult<Option<SessionHold>> {
        if self.elsewhere.lock().unwrap().contains(&id) || !self.here.lock().unwrap().insert(id) {
            return Ok(None);
        }
        Ok(Some(SessionHold::new(HeldHere { here: Arc::clone(&self.here), id })))
    }

    fn forget(&self, id: SessionId) -> StoreResult<()> {
        self.forgotten.lock().unwrap().push(id);
        Ok(())
    }
}

/// A session held by `SharedLocks`, let go as it is dropped.
struct HeldHere {
    here: Arc<Mutex<HashSet<SessionId>>>,
    id: SessionId,
}

impl Drop for HeldHere {
    fn drop(&mut self) {
        self.here.lock().unwrap().remove(&self.id);
    }
}

/// A backend of the entities in memory that behaves as the home store does: it holds sessions by
/// `SharedLocks`, and keeps what lies in a scope, as it tells (`StorePlaces`), below `/data` - a
/// session in `sessions/<id>`, a channel in `channels/<id>` of its session's.
#[derive(Debug)]
struct HomeLikeBackend {
    inner: MemoryStore,
    locks: Arc<SharedLocks>,
}

impl PersistenceBackend for HomeLikeBackend {
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        self.inner.documents(kind, document_of)
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

    fn session_locks(self: Arc<Self>) -> Option<Arc<dyn SessionLocks>> {
        Some(Arc::<SharedLocks>::clone(&self.locks))
    }

    fn places(self: Arc<Self>) -> Option<Arc<dyn StorePlaces>> {
        Some(self)
    }
}

#[async_trait]
impl StorePlaces for HomeLikeBackend {
    fn data_dir(&self) -> &Path {
        Path::new("/data")
    }

    async fn directory_of(&self, scope: &Scope) -> StoreResult<PathBuf> {
        let sessions = self.data_dir().join("sessions");
        Ok(match scope {
            Scope::SessionScope { session_id } => sessions.join(session_id.to_string()),
            Scope::ChannelScope { session_id, channel_id } => {
                sessions.join(format!("{session_id}/channels/{channel_id}"))
            }
            _ => self.data_dir().to_path_buf(),
        })
    }
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

/// What the tests do with a session's participants, channels and threads as the entry points do
/// (issue 593): each a procedure of `procedure::participant`, `channel` or `thread`, under the verb
/// the entry points ask for, failing as the building blocks fail (`SessionError::untyped`).
impl SessionLoop {
    /// Takes `participant` in on behalf of `context`, as `TakeIn::added` does.
    ///
    /// # Errors
    /// As `TakeIn::added`.
    pub async fn add_participant(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        into: Option<(ChannelId, HistoryAccess)>,
    ) -> Result<()> {
        let adding = TakeIn::added(context, self.id(), participant, into);
        self.procedures().run(adding).await.map(drop).map_err(SessionError::untyped)
    }

    /// Takes `participant` in by their own act, as `TakeIn::joining` does.
    ///
    /// # Errors
    /// As `TakeIn::joining`.
    pub async fn join(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        into: Option<ChannelId>,
    ) -> Result<()> {
        let joining = TakeIn::joining(context, self.id(), participant, into);
        self.procedures().run(joining).await.map(drop).map_err(SessionError::untyped)
    }

    /// Lets `participant` go from the session on behalf of `context`, as `LetGo::removed` does.
    ///
    /// # Errors
    /// As `LetGo::removed`.
    pub async fn remove_participant(
        &self,
        context: &SecurityContext,
        participant: ActorId,
    ) -> Result<()> {
        let removing = LetGo::removed(context, self.id(), participant, None);
        self.procedures().run(removing).await.map_err(SessionError::untyped)
    }

    /// Takes `participant` out of the session by their own act, as `LetGo::leaving` does.
    ///
    /// # Errors
    /// As `LetGo::leaving`.
    pub async fn leave(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        let leaving = LetGo::leaving(context, self.id(), participant, None);
        self.procedures().run(leaving).await.map_err(SessionError::untyped)
    }

    /// Creates the channel `spec` asks for on behalf of `context`, as `channel::Create` does.
    ///
    /// # Errors
    /// As `channel::Create`.
    pub async fn create_channel(
        &self,
        context: &SecurityContext,
        spec: ChannelSpec,
    ) -> Result<Arc<ChannelLoop>> {
        let creating = channel::Create::new(context, self.id(), spec);
        self.procedures().run(creating).await.map_err(SessionError::untyped)
    }

    /// Deletes channel `id` on behalf of `context`, as `channel::Delete` does.
    ///
    /// # Errors
    /// As `channel::Delete`.
    pub async fn delete_channel(&self, context: &SecurityContext, id: ChannelId) -> Result<bool> {
        let deleting = channel::Delete::new(context, self.id(), id);
        self.procedures().run(deleting).await.map_err(SessionError::untyped)
    }

    /// Restores the deleted channel `id` on behalf of `context`, as `channel::Restore` does.
    ///
    /// # Errors
    /// As `channel::Restore`.
    pub async fn restore_channel(
        &self,
        context: &SecurityContext,
        id: ChannelId,
    ) -> Result<Arc<ChannelLoop>> {
        let restoring = channel::Restore::new(context, self.id(), id);
        self.procedures().run(restoring).await.map_err(SessionError::untyped)
    }

    /// Opens a thread of `channel` at `origin`, its circle `circle`, on behalf of `context`, with
    /// `founders`, as `thread::Open` does.
    ///
    /// # Errors
    /// As `thread::Open`.
    pub async fn open_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        origin: ThreadOrigin,
        circle: Circle,
        founders: Vec<ActorId>,
    ) -> Result<ThreadId> {
        let opening = thread::Open::new(context, self.id(), channel, origin, circle, founders);
        self.procedures().run(opening).await.map_err(SessionError::untyped)
    }

    /// Takes `participant` into `thread` of `channel` on behalf of `context`, who may change the
    /// channel, as `TakeIntoThread::added` does.
    ///
    /// # Errors
    /// As `TakeIntoThread`.
    pub async fn take_into_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Result<()> {
        let taking_in = TakeIntoThread::added(context, self.id(), channel, thread, participant);
        self.procedures().run(taking_in).await.map_err(SessionError::untyped)
    }

    /// Lets `participant` go from `thread` of `channel` on behalf of `context`, who may change the
    /// channel, as `LetGoFromThread::removed` does.
    ///
    /// # Errors
    /// As `LetGoFromThread`.
    pub async fn let_go_from_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Result<()> {
        let letting_go = LetGoFromThread::removed(context, self.id(), channel, thread, participant);
        self.procedures().run(letting_go).await.map_err(SessionError::untyped)
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - into `thread` of
    /// `channel` by their own act, as `TakeIntoThread::joining` does.
    ///
    /// # Errors
    /// As `TakeIntoThread`.
    pub async fn join_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Result<()> {
        let joining = TakeIntoThread::joining(context, self.id(), channel, thread, participant);
        self.procedures().run(joining).await.map_err(SessionError::untyped)
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - out of `thread` of
    /// `channel` by their own act, as `LetGoFromThread::leaving` does.
    ///
    /// # Errors
    /// As `LetGoFromThread`.
    pub async fn leave_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Result<()> {
        let leaving = LetGoFromThread::leaving(context, self.id(), channel, thread, participant);
        self.procedures().run(leaving).await.map_err(SessionError::untyped)
    }
}
