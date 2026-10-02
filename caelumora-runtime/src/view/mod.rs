//! Views: the sessions, their channels and their members, as one viewer
//! navigates them (issue 163). A view is a node the viewer stands on; its
//! methods are the edges to the next ones, and each edge asks its own
//! access question, in the viewer's name. A node is data the viewer was
//! allowed to reach; it decides nothing - an operation still checks its own
//! permission when it acts (issue 80).
//!
//! What a node shows is what is stored - the documents from the
//! `EntityStore`, the agents' states from the `StateStore` - reached
//! through the running loop where there is one, which holds the truth in
//! memory and writes the store after. So one navigation serves running and
//! stopped sessions alike, and reaching a session does not start it.
//!
//! Two kinds of viewer walk the same edges by two rules. A **user** reads a
//! channel if they may read its document and are cleared for it (issues
//! 70, 133). An **agent** reads *from its channel* (`Viewer::from_place`,
//! issue 17): another channel as a whole if its channel may read it by
//! their classifications (`AccessQuestions::may_channel_read`, issue 133),
//! else entry by entry - only what was released to it - and a channel it
//! may not read at all is one that does not exist. Its root is its own
//! session, reached through its turn (`Turn::session`); a frontend's root
//! is [`SessionManager::views`].
//!
//! A node answers with a snapshot (`caelumora_model::snapshot`, issue 80)
//! for a client that lists and offers: the node's data with its questions
//! answered at one moment.

mod channel;
mod context;
mod member;
mod session;

pub use channel::{ChannelView, MemberView, PublishedNotes};
pub use context::{AgentContextView, ContextError};
#[cfg(test)]
pub(crate) use context::InternalContext;
pub use session::SessionView;

use crate::runtime::{ActorRuntime, RuntimeManager};
use crate::session::{Found, SessionLoop, SessionManager};
use crate::tool::ToolError;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality};
use caelumora_model::channel::Participant;
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::security::{AccessQuestions, Place, SecurityContext};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::StoreError;
use std::sync::Arc;

/// Where an agent reads from: its channel, the classification it reads at
/// (its turn's, issue 67) and its session's privacy (issue 134). The
/// `reader` of every `may_channel_read` it asks.
#[derive(Debug, Clone)]
pub struct ReaderPlace {
    pub channel: ChannelRef,
    pub classification: Classification,
    pub session: SessionPrivacy,
}

impl ReaderPlace {
    fn place(&self) -> Place<'_> {
        Place { channel: &self.channel, classification: &self.classification, session: &self.session }
    }
}

/// Who navigates: in whose name every edge asks its question (issue 131),
/// and - for an agent - from where.
#[derive(Debug, Clone)]
pub struct Viewer {
    context: SecurityContext,
    from: Option<ReaderPlace>,
}

impl Viewer {
    /// A viewer who reads by what they may read: a user.
    pub fn new(context: SecurityContext) -> Self {
        Self { context, from: None }
    }

    /// A viewer who reads from a place: an agent in its channel.
    pub fn from_place(context: SecurityContext, from: ReaderPlace) -> Self {
        Self { context, from: Some(from) }
    }

    pub fn context(&self) -> &SecurityContext {
        &self.context
    }

    /// Where the viewer reads from, if it reads as an agent does.
    pub fn place(&self) -> Option<&ReaderPlace> {
        self.from.as_ref()
    }
}

/// Why an edge could not be followed. The messages are meant for users
/// (see `crate::error::user_message`) - or, through `ToolError`, for the
/// model that asked.
///
/// What does not exist and what the viewer may not read are one case each,
/// so that nothing can be probed - as `SessionError::ChannelNotFound` holds
/// for operations.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("There is no session {0}")]
    SessionNotFound(SessionId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    /// Asked for by name, as an agent's tools do: no such channel, or
    /// nothing in it that can be read from where the viewer reads.
    #[error("there is no channel named \"{0}\" that can be read from here")]
    NoChannelNamed(String),
    #[error("Channel {channel} has no member {actor}")]
    MemberNotFound { channel: ChannelId, actor: ActorId },
    /// Reached, but not the viewer's to read: what an agent saw.
    #[error("You are not permitted to see what {actor} saw")]
    Forbidden { actor: ActorId },
    /// Only agents have a context.
    #[error("There is nothing {actor} saw: it is no agent, or no longer exists")]
    NoContext { actor: ActorId },
    /// Reaching it failed; the cause is in the log.
    #[error("Reading failed")]
    Failed(#[source] anyhow::Error),
}

pub type ViewResult<T> = std::result::Result<T, ViewError>;

/// What a model is told when a tool of its agent could not follow an edge:
/// what cannot be reached is the model's to correct; a failure is the
/// tool's.
impl From<ViewError> for ToolError {
    fn from(error: ViewError) -> Self {
        match error {
            ViewError::Forbidden { .. } => ToolError::PermissionDenied(error.to_string()),
            ViewError::Failed(_) => ToolError::ExecutionFailed(error.to_string()),
            _ => ToolError::InvalidParameter(error.to_string()),
        }
    }
}

/// Where the running sessions are: a root of views reaches a session
/// through its loop if it runs. The `SessionManager` knows them all; an
/// agent knows its own.
pub(crate) trait RunningSessions: Send + Sync {
    fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>>;
}

impl RunningSessions for SessionManager {
    fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        SessionManager::running(self, id)
    }
}

/// The one session an agent is in.
struct OwnSession(Arc<SessionLoop>);

impl RunningSessions for OwnSession {
    fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        (self.0.id() == id).then(|| self.0.clone())
    }
}

/// The root of the navigation: every session the viewer may read.
#[derive(Clone)]
pub struct Views {
    runtime: Arc<RuntimeManager>,
    running: Arc<dyn RunningSessions>,
    viewer: Viewer,
}

impl Views {
    pub(crate) fn new(runtime: Arc<RuntimeManager>, running: Arc<dyn RunningSessions>, viewer: Viewer) -> Self {
        Self { runtime, running, viewer }
    }

    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    pub(crate) fn runtime(&self) -> &RuntimeManager {
        &self.runtime
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        let sessions = self.runtime.entity_store().list_sessions(self.viewer.context()).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the sessions")))?;
        // The store lists what the viewer may read; a running session is
        // reached through its loop.
        Ok(sessions.into_iter()
            .map(|session| match self.running.running(session.id) {
                Some(session_loop) => Found::Running(session_loop),
                None => Found::Stored(session),
            })
            .map(|found| SessionView::of(self.clone(), found))
            .collect())
    }

    /// Session `id`, if the viewer may read it (`GetSession`).
    ///
    /// # Errors
    /// `SessionNotFound` if there is no such session or the viewer may not
    /// read it.
    pub async fn session(&self, id: SessionId) -> ViewResult<SessionView> {
        let not_found = || ViewError::SessionNotFound(id);
        let found = match self.running.running(id) {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                if !self.runtime.security_manager().may_read_session(self.viewer.context(), &attributes).await {
                    return Err(not_found());
                }
                Found::Running(session_loop)
            }
            None => Found::Stored(self.runtime.entity_store().get_session(self.viewer.context(), id).await.map_err(|e| {
                let e = anyhow::Error::new(e);
                if crate::error::is_not_found(&e, "session") || crate::error::is_forbidden(&e) { not_found() } else { ViewError::Failed(e) }
            })?),
        };
        Ok(SessionView::of(self.clone(), found))
    }

    /// The channel `channel` names, wherever it is - what a memory entry's
    /// origin or a `KnowledgeRef` points to - if the viewer may read it:
    /// `None` if its session or the channel is gone, or not the viewer's to
    /// read (asked as `session(id)` and `SessionView::channel` ask). Until
    /// the store keeps what was deleted (issue 164), gone and never there
    /// are one answer.
    ///
    /// # Errors
    /// `Failed` if reaching it failed.
    pub async fn channel_of(&self, channel: ChannelRef) -> ViewResult<Option<ChannelView>> {
        let session = match self.session(channel.session_id).await {
            Ok(session) => session,
            Err(ViewError::SessionNotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        match session.channel(channel.channel_id).await {
            Ok(channel) => Ok(Some(channel)),
            Err(ViewError::ChannelNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether the viewer, reading from its place, may read what is
    /// classified `classification` in `channel` of a session with privacy
    /// `session` (`AccessQuestions::may_channel_read`, issue 133). A viewer
    /// without a place reads by other questions and is not asked this one.
    pub(crate) async fn may_read_from(&self, channel: &ChannelRef, classification: &Classification, session: &SessionPrivacy) -> bool {
        let Some(from) = &self.viewer.from else { return true };
        let source = Place { channel, classification, session };
        self.runtime.security_manager().may_channel_read(self.viewer.context(), from.place(), source).await
    }

    /// The name of `participant`; `None` for a user or agent that no
    /// longer exists. Read in the runtime's name: taking part in a session
    /// makes a name known to everybody in it - it stands at every message -
    /// even where the record itself is not theirs to read (issue 120).
    async fn name_of(&self, participant: &Participant) -> Option<String> {
        match participant {
            Participant::User(id) => self.runtime.entity_store().get_user(&SecurityContext::system(), *id).await.ok().map(|user| user.username),
            Participant::Agent(id) => self.agent_of(*id).await.map(|agent| agent.name.to_string()),
            Participant::Approver(_) => participant.actor_id().fixed_name().map(str::to_string),
        }
    }

    /// The document of agent `id` - its name, its attributes, its
    /// personality - or `None` if it no longer exists. Read in the
    /// runtime's name, like its name.
    async fn agent_of(&self, id: AgentId) -> Option<Agent> {
        self.runtime.entity_store().get_agent(&SecurityContext::system(), id).await.ok()
    }

    /// The personality of `agent` - what kind of agent it is (issue 179),
    /// how it is introduced (issue 176) - or `None` if it no longer exists.
    /// Read in the runtime's name: introductions are public, and the kind
    /// decides how the agent's state is read, not what the viewer may see.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    async fn personality_of(&self, agent: &Agent) -> ViewResult<Option<Personality>> {
        match self.runtime.entity_store().get_personality(&SecurityContext::system(), agent.personality_id).await {
            Ok(personality) => Ok(Some(personality)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(ViewError::Failed(anyhow::Error::new(e).context(format!("reading the personality of agent {}", agent.id)))),
        }
    }

    /// The session `id` as privacy sees it now, or `None` if there is no
    /// such session any more (issue 134). Not an edge: looked up in the
    /// runtime's name, for what an agent may recall of its memory from
    /// another session - the runtime is asked to enforce what that
    /// session's people agreed to, and nothing else of it reaches the
    /// viewer. Changes to a session are stored before they take effect, so
    /// the stored one is the current one, running or not.
    ///
    /// # Errors
    /// `Failed` if it cannot be looked up.
    pub(crate) async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>> {
        match self.runtime.entity_store().get_session(&SecurityContext::system(), id).await {
            Ok(session) => Ok(Some(SessionPrivacy::of(&session))),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(ViewError::Failed(anyhow::Error::new(e).context(format!("looking up session {id}")))),
        }
    }

    /// The roles of the viewer's user, which decide what they are cleared
    /// for (issue 133). Without them, only what is public: a failed lookup
    /// shows less.
    async fn roles(&self) -> Vec<String> {
        crate::channel::user_roles(self.runtime.entity_store().as_ref(), self.viewer.context().initiator_id).await.unwrap_or_default()
    }
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Views").field("viewer", &self.viewer).finish_non_exhaustive()
    }
}

/// What an agent's views are built from, once per turn (`Turn::session`):
/// where its session is reached, and its context. The place it reads from
/// is completed with the turn's channel and classification when the views
/// are built.
#[derive(Clone)]
pub struct ViewsFactory {
    root: FactoryRoot,
    context: SecurityContext,
}

/// Where a factory reaches the agent's session.
#[derive(Clone)]
enum FactoryRoot {
    /// Through the actor's runtime, which reaches its channel and session
    /// once they are attached.
    Actor(ActorRuntime),
    /// A session loop held directly: for turns outside of an actor, in
    /// tests.
    #[cfg(test)]
    Session { session: Arc<SessionLoop>, runtime: Arc<RuntimeManager> },
}

impl ViewsFactory {
    pub(crate) fn of_actor(runtime: ActorRuntime, context: SecurityContext) -> Self {
        Self { root: FactoryRoot::Actor(runtime), context }
    }

    #[cfg(test)]
    pub(crate) fn in_session(session: Arc<SessionLoop>, runtime: Arc<RuntimeManager>, context: SecurityContext) -> Self {
        Self { root: FactoryRoot::Session { session, runtime }, context }
    }

    /// The agent's own session, as it reads from `channel` at
    /// `classification`; `None` while the channel is not attached to its
    /// session yet, or once it is gone.
    pub(crate) fn session(&self, channel: ChannelRef, classification: Classification) -> Option<SessionView> {
        let (session_loop, runtime) = match &self.root {
            FactoryRoot::Actor(actor) => (actor.channel_loop()?.session_loop()?, actor.runtime_manager().clone()),
            #[cfg(test)]
            FactoryRoot::Session { session, runtime } => (session.clone(), runtime.clone()),
        };
        let from = ReaderPlace { channel, classification, session: session_loop.privacy() };
        let views = Views::new(runtime, Arc::new(OwnSession(session_loop.clone())), Viewer::from_place(self.context.clone(), from));
        Some(SessionView::of(views, Found::Running(session_loop)))
    }
}

impl std::fmt::Debug for ViewsFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewsFactory").field("context", &self.context).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::{Name, ScopedName};
    use crate::runtime::RuntimeManager;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::channel::Channel;
    use caelumora_model::command::default_commands;
    use caelumora_model::confidentiality::{Classification, Visibility};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{OwnershipSecurityManager, SecurityAttributes, SecurityManager};
    use caelumora_model::session::Session;
    use caelumora_model::store::EntityStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::{User, UserId};

    /// A runtime under the ownership policy, and a manager on it.
    fn owned_runtime() -> (Arc<MemoryStore>, Arc<SessionManager>) {
        owned_runtime_under(Arc::new(OwnershipSecurityManager::new()))
    }

    fn owned_runtime_under(policy: Arc<dyn SecurityManager>) -> (Arc<MemoryStore>, Arc<SessionManager>) {
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        (store, Arc::new(SessionManager::new(runtime)))
    }

    fn user(name: &str) -> User {
        User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() }
    }

    fn as_user(id: UserId) -> Viewer {
        Viewer::new(SecurityContext::new(ActorId::User(id), Default::default(), Default::default()))
    }

    async fn record_users(store: &MemoryStore, users: &[&User]) -> Vec<Participant> {
        let mut participants = Vec::new();
        for user in users {
            store.create_user(&SecurityContext::system(), user).await.unwrap();
            participants.push(Participant::User(user.id));
        }
        participants
    }

    fn make_session(name: &str, owner: ActorId, contributors: Vec<ActorId>) -> Session {
        Session {
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId::new(),
            name: name.to_string(),
            participants: Vec::new(),
            channels: vec![ChannelId::new()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes { owner_id: owner, contributors },
        }
    }

    fn main_channel(session: &Session) -> Channel {
        Channel {
            id: session.channels[0],
            session_id: session.id,
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
            participants: session.participants.clone(),
            security_attributes: session.security_attributes.clone(),
        }
    }

    async fn an_agent_of(store: &MemoryStore, owner: ActorId) -> Agent {
        an_agent_of_kind(store, owner, Default::default()).await
    }

    async fn an_agent_of_kind(store: &MemoryStore, owner: ActorId, kind: caelumora_model::agent::PersonalityKind) -> Agent {
        let personality = Personality {
            description: String::new(),
            introduction: None,
            reasoning_visibility: Visibility::Channel,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind,
            roles: None,
            components: Default::default(),
            security_attributes: Default::default(),
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Helper", Scope::GlobalScope).unwrap(), &personality, owner);
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_context_of_an_external_agent_is_what_the_remote_agent_was_given() {
        use caelumora_model::agent::PersonalityKind;
        use caelumora_model::external_agent::ExternalAgentChannelState;
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let remote = an_agent_of_kind(&store, ActorId::User(alice.id), PersonalityKind::External).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(remote.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(remote.id));
        let channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();

        let said = |t: &str| Message { id: MessageId::new(), content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (question, answer) = (said("question"), said("answer"));
        let mut state = ExternalAgentChannelState::default();
        state.history.extend(vec![question.clone(), answer.clone()]);
        state.transcript.extend(vec![question.clone()]);
        store.put_external_agent_channel_state(remote.id, channel.id, &state).await.unwrap();
        // What an agent run here would have kept: not the external agent's.
        let mut stray = caelumora_model::agent::AgentChannelState::default();
        stray.notes.push(Change::Add { id: NoteId(1), text: "stray".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        stray.private_notes.push(Change::Add { id: NoteId(1), text: "stray".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(remote.id, channel.id, &stray).await.unwrap();

        let for_alice = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let member = for_alice.member(ActorId::Agent(remote.id)).await.unwrap();
        let seen = member.context().await.expect("alice made the agent");
        let texts = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        assert_eq!(texts(seen.transcript().messages()), ["question"], "what it was given, not what it answered");
        assert_eq!(seen.private_notes(), (Vec::new(), None), "an external agent writes none");
        assert!(seen.given_up_turns().is_empty());
        assert!(matches!(seen.context_of(answer.id), Err(ContextError::NotRecorded { .. })), "what it made of it is the remote agent's");
        assert_eq!(seen.classification_of(answer.id), Some(Classification::CHANNEL_DEFAULT), "its answer is in its history");
        assert_eq!(member.private_notes().await.unwrap(), (Vec::new(), None));
        assert!(member.notes().await.unwrap().expect("an agent").notes.is_empty(), "none, though an agent run here kept some");
        assert!(for_alice.notes().await.unwrap().iter().all(|published| published.notes.is_empty()));
    }

    #[tokio::test]
    async fn the_views_show_what_their_viewer_may_read_and_do_whether_the_session_runs_or_not() {
        let (store, manager) = owned_runtime();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;

        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::User(bob.id), ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let main = main_channel(&session);
        let running = manager.create_session(as_user(alice.id).context(), session, vec![main]).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = running.create_channel(as_user(alice.id).context(), crate::session::ChannelSpec { name: Name::new("private").unwrap(), components: Default::default(), classification: None, participants: vec![] }).await.unwrap().id();
        let id = running.id();

        let check = |state: &'static str| {
            let manager = &manager;
            async move {
                let names = |channels: &[ChannelView]| channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>();
                let for_alice = manager.views(as_user(alice.id)).session(id).await.unwrap();
                assert_eq!(names(&for_alice.channels().await), ["main", "private"], "{state}: the owner reads every channel");
                assert_eq!(for_alice.snapshot().await.participants.iter().filter_map(|p| p.name.as_deref()).collect::<Vec<_>>(), ["alice", "bob", "Helper"], "{state}");
                let for_bob = manager.views(as_user(bob.id)).session(id).await.unwrap();
                assert_eq!(names(&for_bob.channels().await), ["main"], "{state}: bob is not in the private channel");
                assert!(for_alice.may_change().await && !for_bob.may_change().await, "{state}: the owner changes the session, a participant does not");

                let main = for_alice.channel(for_alice.channels().await[0].id()).await.unwrap();
                assert!(main.may_change().await, "{state}: the owner changes the channel");
                let agent = main.member(ActorId::Agent(helper.id)).await.expect("the agent is a member");
                assert_eq!(agent.handle().as_deref(), Some("helper"), "{state}");
                assert!(agent.may_read_context().await, "{state}: alice made the agent");
                let main = &for_bob.channels().await[0];
                assert!(!main.may_change().await, "{state}: a participant does not change it");
                for member in main.members().await {
                    assert!(!member.may_read_context().await, "{state}: bob reads no agent's context");
                }

                let hidden = for_bob.channel(hers).await.expect_err("not bob's to read");
                assert!(matches!(hidden, ViewError::ChannelNotFound { .. }), "{state}: absent, not refused: {hidden}");
                assert_eq!(for_alice.channel(hers).await.unwrap().name(), "private", "{state}");
                let refused = manager.views(as_user(carol.id)).session(id).await.expect_err("carol may not see it");
                assert!(matches!(refused, ViewError::SessionNotFound(_)), "{state}: {refused}");
                assert!(manager.views(as_user(carol.id)).sessions().await.unwrap().is_empty(), "{state}: nor is it listed for her");
                let listed = manager.views(as_user(bob.id)).sessions().await.unwrap();
                assert_eq!(listed.len(), 1, "{state}");
                assert_eq!(listed[0].snapshot().await, for_bob.snapshot().await, "{state}: listed as it is reached");
            }
        };
        check("running").await;
        assert!(manager.stop_session(&SecurityContext::system(), id).await.unwrap(), "it ran");
        check("stopped").await;
        assert!(manager.running(id).is_none(), "reaching a session does not start it");
    }

    #[tokio::test]
    async fn a_channel_is_described_with_handles_only_for_who_can_be_mentioned() {
        use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};

        let (store, manager) = owned_runtime();
        let alice = User { id: UserId::new(), username: "Alice Example".to_string(), email: String::new(), roles: Vec::new() };
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut session = make_session("s", ActorId::User(alice.id), vec![]);
        session.participants = record_users(&store, &[&alice]).await;
        let mut channel = main_channel(&session);
        channel.participants.extend([Participant::User(gone), Participant::Approver(approver)]);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap().snapshot().await;

        let [alice, gone, approver] = view.members.as_slice() else { panic!("three members, in order: {view:?}") };
        assert_eq!((alice.name.as_deref(), alice.handle.as_deref()), (Some("Alice Example"), Some("alice-example")));
        assert_eq!((gone.name.as_deref(), gone.handle.as_deref()), (None, None), "a deleted user is listed, but cannot be mentioned");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
        assert!(view.members.iter().all(|m| !m.can_read_context), "no member is an agent");
    }

    #[tokio::test]
    async fn what_a_channel_holds_is_read_through_its_nodes_and_a_members_context_by_who_may() {
        use caelumora_model::channel::{ChannelState, HistoryLimit, HistoryStart};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::User(bob.id), ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let mut channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();

        // What was said, of which bob - who joined later - sees the end only.
        let said = |t: &str| Message { content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (first, second) = (said("first"), said("second"));
        channel.history_limits.push(HistoryLimit { participant: ActorId::User(bob.id), start: HistoryStart::After(first.id) });
        // And so does the agent, though alice, for whom it acts, sees it all.
        channel.history_limits.push(HistoryLimit { participant: ActorId::Agent(helper.id), start: HistoryStart::After(first.id) });
        store.create_channel(&system, &channel).await.unwrap();
        let mut state = ChannelState::new(channel.id);
        state.history.extend(vec![first.clone(), second.clone()]);
        store.put_channel_state(channel.id, &state).await.unwrap();
        // What the agent noted and saw.
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        agent_state.notes.push(Change::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.private_notes.push(Change::Add { id: NoteId(1), text: "mine".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.transcript.push(first.clone());
        store.put_agent_channel_state(helper.id, channel.id, &agent_state).await.unwrap();

        let reach = |viewer: Viewer| {
            let manager = &manager;
            async move { manager.views(viewer).session(session.id).await.unwrap().channel(channel.id).await.unwrap() }
        };
        let for_alice = reach(as_user(alice.id)).await;
        let texts = |messages: &[Message]| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&for_alice.history().await.unwrap().messages), ["first", "second"]);
        assert_eq!(texts(&reach(as_user(bob.id)).await.history().await.unwrap().messages), ["second"], "bob joined after the first");
        let as_helper = Viewer::new(SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(ActorId::Agent(helper.id)));
        assert_eq!(texts(&reach(as_helper).await.history().await.unwrap().messages), ["second"], "the agent reads with its own limit, not its owner's");
        let notes = for_alice.notes().await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].agent, notes[0].name.as_str(), notes[0].notes[0].text.as_str()), (ActorId::Agent(helper.id), "Helper", "noted"));

        let agent = for_alice.member(ActorId::Agent(helper.id)).await.unwrap();
        let seen = agent.context().await.expect("alice made the agent");
        assert_eq!(seen.private_notes().0[0].text, "mine");
        assert_eq!(seen.transcript().messages().len(), 1);
        assert!(matches!(seen.context_of(first.id), Err(ContextError::NotRecorded { .. })), "the agent did not produce it");
        let refused = reach(as_user(bob.id)).await.member(ActorId::Agent(helper.id)).await.unwrap().context().await.expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let none = for_alice.member(ActorId::User(bob.id)).await.unwrap().context().await.expect_err("a user saw nothing");
        assert!(matches!(none, ViewError::NoContext { .. }), "{none}");
        assert!(matches!(for_alice.member(ActorId::System).await, Err(ViewError::MemberNotFound { .. })));
    }

    #[tokio::test]
    async fn the_approvals_a_channel_waits_for_are_read_from_its_agents_turns() {
        use caelumora_model::actor::ActorState;
        use caelumora_model::agent::{AgentChannelState, TurnState};
        use caelumora_model::store::StateStore;
        use caelumora_model::tool::{ToolCall, ToolCallState};

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

        // The agent's turn waits for one call, the other was approved.
        let call = |name: &str| ToolCall { id: Some(name.to_string()), function_name: name.to_string(), arguments: serde_json::json!({}), visibility: Default::default() };
        let waiting = ToolCallState::pending(call("delete"), "Delete it?".to_string());
        let mut turn = TurnState::default();
        turn.event_context.session_id = session.id;
        turn.event_context.channel_id = channel.id;
        turn.event_context.sender_id = ActorId::Agent(helper.id);
        turn.tool_calls = vec![waiting.clone(), ToolCallState::approved(call("echo"))];
        let state = AgentChannelState { processing_state: ActorState::WaitingForApproval, turn: Some(turn), ..Default::default() };
        store.put_agent_channel_state(helper.id, channel.id, &state).await.unwrap();

        let main = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let pending = main.pending_approvals().await.unwrap();
        assert_eq!(pending.len(), 1, "only the call that waits");
        assert_eq!((pending[0].id, pending[0].agent_id, pending[0].channel_id, pending[0].description.as_str()), (waiting.id, helper.id, channel.id, "Delete it?"));
        assert!(manager.running(session.id).is_none(), "read without starting the session");
    }

    #[tokio::test]
    async fn an_agent_reads_another_channel_as_a_whole_by_their_classifications_or_what_was_released_to_it() {
        use caelumora_model::channel::ChannelState;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::privacy::SessionPrivacy;
        use caelumora_model::store::StateStore;

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let (store, manager) = owned_runtime_under(Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments))));
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let system = SecurityContext::system();
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let mut channel_of = |name: &str, classification: Classification| {
            let channel = Channel { id: ChannelId::new(), name: Name::new(name).unwrap(), classification, ..template.clone() };
            session.channels.push(channel.id);
            channel
        };
        let public = channel_of("public", classified(Confidentiality::Public, &[]));
        let team = channel_of("team", classified(Confidentiality::Internal, &[]));
        let secret = channel_of("secret", classified(Confidentiality::Confidential, &[]));
        let hr = channel_of("hr", classified(Confidentiality::Internal, &["hr"]));
        store.create_session(&system, &session).await.unwrap();
        for channel in [&public, &team, &secret, &hr] {
            store.create_channel(&system, channel).await.unwrap();
        }

        // Said and noted in secret: one of each released to everyone.
        let said = |t: &str, classification: &Classification| Message { id: MessageId::new(), content: Some(t.to_string()), ..Message::new(classification.clone()) };
        let mut state = ChannelState::new(secret.id);
        state.history.extend(vec![said("the plan", &secret.classification), said("the date", &public.classification)]);
        store.put_channel_state(secret.id, &state).await.unwrap();
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        let note = |id, text: &str, classification: &Classification| Change::Add { id: NoteId(id), text: text.to_string(), classification: classification.clone() };
        agent_state.notes.push(note(1, "noted secretly", &secret.classification), NoteOrigin::default(), MessageId::default());
        agent_state.notes.push(note(2, "noted openly", &public.classification), NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(helper.id, secret.id, &agent_state).await.unwrap();
        let mut state = ChannelState::new(team.id);
        state.history.extend(vec![said("internal", &team.classification)]);
        store.put_channel_state(team.id, &state).await.unwrap();

        // The agent, reading from one of its channels, as its components do.
        let reading_from = |channel: &Channel| {
            let context = SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(ActorId::Agent(helper.id));
            let from = ReaderPlace { channel: ChannelRef { session_id: session.id, channel_id: channel.id }, classification: channel.classification.clone(), session: SessionPrivacy::of(&session) };
            Viewer::from_place(context, from)
        };
        let names = |channels: Vec<ChannelView>| channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>();
        let texts = |history: crate::channel::VisibleHistory| history.messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();

        let from_team = manager.views(reading_from(&team)).session(session.id).await.unwrap();
        assert_eq!(names(from_team.channels().await), ["public", "team"], "as a whole: what is public, and itself");
        assert_eq!(names(from_team.all_channels().await), ["public", "team", "secret", "hr"], "the rest entry by entry");
        let secret_from_team = from_team.channel_named("secret").await.expect("reachable for what was released");
        assert_eq!(texts(secret_from_team.history().await.unwrap()), ["the date"]);
        let notes = secret_from_team.notes().await.unwrap();
        assert_eq!(notes[0].notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["noted openly"]);
        let refused = secret_from_team.member(ActorId::Agent(helper.id)).await.unwrap().context().await.expect_err("not as a whole");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let hr_from_team = from_team.channel_named("hr").await.unwrap();
        assert!(matches!(hr_from_team.history().await, Err(ViewError::NoChannelNamed(name)) if name == "hr"), "nothing released: as if it did not exist");
        assert!(matches!(hr_from_team.notes().await, Err(ViewError::NoChannelNamed(_))));
        assert!(matches!(from_team.channel_named("nope").await, Err(ViewError::NoChannelNamed(_))));

        let from_secret = manager.views(reading_from(&secret)).session(session.id).await.unwrap();
        assert_eq!(names(from_secret.channels().await), ["public", "team", "secret"], "confidential reads internal; HR's never a general channel");
        assert_eq!(texts(from_secret.channel_named("team").await.unwrap().history().await.unwrap()), ["internal"]);
        let whole = from_secret.channel(secret.id).await.unwrap();
        assert_eq!(texts(whole.history().await.unwrap()), ["the plan", "the date"], "a channel reads itself as a whole");
        assert!(whole.member(ActorId::Agent(helper.id)).await.unwrap().context().await.is_ok());
    }

    #[tokio::test]
    async fn a_member_that_left_is_still_reached_by_what_it_said_and_a_gone_channel_is_none() {
        use caelumora_model::channel::ChannelState;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        // The agent took part, said something, and was removed.
        let mut channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        let mut state = ChannelState::new(channel.id);
        state.history.extend(vec![Message { id: MessageId::new(), sender_id: ActorId::Agent(helper.id), content: Some("I was here".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }]);
        store.put_channel_state(channel.id, &state).await.unwrap();

        let views = manager.views(as_user(alice.id));
        let reached = views.channel_of(ChannelRef { session_id: session.id, channel_id: channel.id }).await.unwrap().expect("there");
        let former = reached.member(ActorId::Agent(helper.id)).await.expect("known by what it said");
        assert!(former.former() && former.name() == Some("Helper"));
        assert!(former.context().await.is_ok(), "what it saw is still the owner's to read");
        assert!(reached.members().await.iter().all(|m| m.id() != ActorId::Agent(helper.id)), "not listed among the members");
        assert!(matches!(reached.member(ActorId::User(UserId::new())).await, Err(ViewError::MemberNotFound { .. })), "a user who never took part");

        // Gone: the channel, then the session.
        channel.id = ChannelId::new();
        assert!(views.channel_of(ChannelRef { session_id: session.id, channel_id: channel.id }).await.unwrap().is_none());
        assert!(views.channel_of(ChannelRef { session_id: SessionId::new(), channel_id: channel.id }).await.unwrap().is_none());
        assert!(manager.views(as_user(UserId::new())).channel_of(ChannelRef { session_id: session.id, channel_id: reached.id() }).await.unwrap().is_none(), "nor is it there for a stranger");
    }
}
