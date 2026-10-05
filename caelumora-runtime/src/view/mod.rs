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

pub use channel::{ChannelView, MemberView};
pub use context::{AgentContextView, ContextError};
pub use session::SessionView;

use crate::runtime::{ActorRuntime, RuntimeManager};
use crate::session::{Found, SessionLoop, SessionManager};
use crate::tool::ToolError;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::channel::Participant;
use caelumora_model::confidentiality::Classification;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::removal::Removal;
use caelumora_model::security::{AccessQuestions, Place, SecurityContext, SecurityManager};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::{AgentSnapshot, DeletedSnapshot};
use caelumora_model::store::{EntityKind, EntityStore, StateStore, StoreResultExt};
use crate::component::ComponentRegistry;
use std::sync::Arc;

/// Who navigates: in whose name every edge asks its question (issue 131),
/// and - for an agent - from where.
#[derive(Debug, Clone)]
pub struct Viewer {
    context: SecurityContext,
    /// Where an agent reads from: its channel, the classification it reads
    /// at (its turn's, issue 67) and its session's privacy (issue 134) -
    /// the `reader` of every `may_channel_read` it asks.
    from: Option<Place>,
}

impl Viewer {
    /// A viewer who reads by what they may read: a user.
    pub fn new(context: SecurityContext) -> Self {
        Self { context, from: None }
    }

    /// A viewer who reads from a place: an agent in its channel.
    pub fn from_place(context: SecurityContext, from: Place) -> Self {
        Self { context, from: Some(from) }
    }

    pub fn context(&self) -> &SecurityContext {
        &self.context
    }

    /// Where the viewer reads from, if it reads as an agent does.
    pub fn place(&self) -> Option<&Place> {
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
    /// Reached, but not the viewer's to read: what an agent saw, which
    /// tools it has.
    #[error("You are not permitted to read this of {actor}")]
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
        (self.0.id() == id).then(|| Arc::clone(&self.0))
    }
}

/// The root of the navigation: every session the viewer may read. Holds
/// the services every edge uses, taken from the runtime once (issue 229).
#[derive(Clone)]
pub struct Views {
    security: Arc<dyn SecurityManager>,
    entities: Arc<dyn EntityStore>,
    states: Arc<dyn StateStore>,
    components: Arc<ComponentRegistry>,
    running: Arc<dyn RunningSessions>,
    viewer: Viewer,
}

impl Views {
    pub(crate) fn new(runtime: &RuntimeManager, running: Arc<dyn RunningSessions>, viewer: Viewer) -> Self {
        Self {
            security: Arc::clone(runtime.security_manager()),
            entities: Arc::clone(runtime.entity_store()),
            states: Arc::clone(runtime.state_store()),
            components: Arc::clone(runtime.component_registry()),
            running,
            viewer,
        }
    }

    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    /// The views of an agent in `session`, which it reaches alone (issue
    /// 229): what `SessionLoop::views` gives.
    pub(crate) fn of_own_session(runtime: &RuntimeManager, session: Arc<SessionLoop>, viewer: Viewer) -> Self {
        Self::new(runtime, Arc::new(OwnSession(session)), viewer)
    }

    /// The viewer's context: in whose name every edge asks.
    pub(crate) fn context(&self) -> &SecurityContext {
        self.viewer.context()
    }

    /// The policy every edge asks, in the viewer's `context`.
    pub(crate) fn security(&self) -> &Arc<dyn SecurityManager> {
        &self.security
    }

    pub(crate) fn entities(&self) -> &Arc<dyn EntityStore> {
        &self.entities
    }

    pub(crate) fn states(&self) -> &Arc<dyn StateStore> {
        &self.states
    }

    pub(crate) fn components(&self) -> &ComponentRegistry {
        &self.components
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        let sessions = self.entities.list_sessions(self.viewer.context()).await
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
                if !self.security.may_read_session(self.viewer.context(), &attributes).await {
                    return Err(not_found());
                }
                Found::Running(session_loop)
            }
            None => Found::Stored(self.entities.get_session(self.viewer.context(), id).await.map_err(|e| {
                let e = anyhow::Error::new(e);
                if crate::error::is_not_found(&e, "session") || crate::error::is_forbidden(&e) { not_found() } else { ViewError::Failed(e) }
            })?),
        };
        Ok(SessionView::of(self.clone(), found))
    }

    /// The channel `channel` of session `session`, wherever it is - what a
    /// memory entry's origin or a `KnowledgeRef` points to (issue 164):
    /// there, if the viewer may read it (asked as `session(id)` and
    /// `SessionView::channel` ask); gone, if it or its session was deleted
    /// and the viewer could read it were it there (`GetChannel`,
    /// `GetSession` on its stored attributes), with who deleted it and
    /// when; never there otherwise - so nothing can be probed.
    ///
    /// # Errors
    /// `Failed` if reaching it failed.
    pub async fn channel_of(&self, session: SessionId, channel: ChannelId) -> ViewResult<Reached<ChannelView>> {
        let session_view = match self.session(session).await {
            Ok(session_view) => session_view,
            Err(ViewError::SessionNotFound(_)) => return self.gone_session(session).await,
            Err(e) => return Err(e),
        };
        match session_view.channel(channel).await {
            Ok(channel) => Ok(Reached::There(channel)),
            Err(ViewError::ChannelNotFound { .. }) => self.gone_channel(session, channel).await,
            Err(e) => Err(e),
        }
    }

    /// Whether session `id` was deleted and the viewer could read it were
    /// it there.
    async fn gone_session<T>(&self, id: SessionId) -> ViewResult<Reached<T>> {
        let deleted = self.entities.get_deleted_session(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("looking up session {id}"))))?;
        Ok(match deleted {
            Some(session) if self.security.may_read_session(self.viewer.context(), &session.security_attributes).await =>
                session.deleted.map_or(Reached::Never, Reached::Gone),
            _ => Reached::Never,
        })
    }

    /// Whether channel `id` of session `session` was deleted and the viewer
    /// could read it were it there.
    async fn gone_channel<T>(&self, session: SessionId, id: ChannelId) -> ViewResult<Reached<T>> {
        let deleted = self.entities.get_deleted_channel(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("looking up channel {id}"))))?;
        Ok(match deleted {
            Some(channel) if channel.session_id == session
                && self.security.may_read_channel(self.viewer.context(), &channel.security_attributes).await =>
                channel.deleted.map_or(Reached::Never, Reached::Gone),
            _ => Reached::Never,
        })
    }

    /// The agents the viewer may read (issue 164), in the order the store
    /// lists them, with whether they may delete them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn agents(&self) -> ViewResult<Vec<AgentSnapshot>> {
        let agents = self.entities.list_agents(self.viewer.context()).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the agents")))?;
        let policy = &self.security;
        let mut listed = Vec::with_capacity(agents.len());
        for agent in agents {
            let can_delete = policy.may_delete_agent(self.viewer.context(), &agent.security_attributes).await;
            listed.push(AgentSnapshot { id: agent.id, name: agent.name.to_string(), can_delete });
        }
        Ok(listed)
    }

    /// The agents the viewer deleted and may read as such (issue 164): the
    /// trash of agents. An agent is purged as the policy allows.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_agents(&self) -> ViewResult<Vec<DeletedSnapshot<AgentId>>> {
        let agents = self.entities.list_deleted_agents(self.viewer.context()).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted agents")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(agents.len());
        for agent in agents {
            let Some(removal) = agent.deleted.clone() else { continue };
            let attributes = &agent.security_attributes;
            deleted.push(DeletedSnapshot {
                id: agent.id,
                name: agent.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Agent, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Agent, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// The sessions the viewer deleted and may read as such (issue 164),
    /// in the order the store lists them: the trash.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_sessions(&self) -> ViewResult<Vec<DeletedSnapshot<SessionId>>> {
        let sessions = self.entities.list_deleted_sessions(self.viewer.context()).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted sessions")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(sessions.len());
        for session in sessions {
            let Some(removal) = session.deleted.clone() else { continue };
            let attributes = &session.security_attributes;
            deleted.push(DeletedSnapshot {
                id: session.id,
                name: session.name.clone(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Session, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Session, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// Whether the viewer, reading from its place, may read what `source`
    /// holds (`AccessQuestions::may_channel_read`, issue 133). A viewer
    /// without a place reads by other questions and is not asked this one.
    pub(crate) async fn may_read_from(&self, source: &Place) -> bool {
        let Some(from) = &self.viewer.from else { return true };
        self.security.may_channel_read(self.viewer.context(), from, source).await
    }

    /// The name of `participant`; `None` for a user or agent that no
    /// longer exists. Read in the runtime's name: taking part in a session
    /// makes a name known to everybody in it - it stands at every message -
    /// even where the record itself is not theirs to read (issue 120).
    async fn name_of(&self, participant: &Participant) -> Option<String> {
        match participant {
            Participant::User(id) => self.entities.get_user(&SecurityContext::system(), *id).await.ok().map(|user| user.username),
            Participant::Agent(id) => match self.agent_of(*id).await {
                Some(agent) => Some(agent.name.to_string()),
                None => self.deleted_agent_name(*id).await,
            },
            Participant::Approver(_) => participant.actor_id().fixed_name().map(str::to_string),
        }
    }

    /// The name of agent `id` if it was deleted and not purged (issue 164):
    /// what it said keeps its sender. Read in the runtime's name, like its
    /// name while it exists.
    pub(super) async fn deleted_agent_name(&self, id: AgentId) -> Option<String> {
        self.entities.get_deleted_agent(&SecurityContext::system(), id).await.ok().map(|agent| agent.name.to_string())
    }

    /// The document of agent `id` - its name, its attributes, its
    /// personality - or `None` if it no longer exists. Read in the
    /// runtime's name, like its name.
    async fn agent_of(&self, id: AgentId) -> Option<Agent> {
        self.entities.get_agent(&SecurityContext::system(), id).await.ok()
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
        let session = self.entities.get_session(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("looking up session {id}"))))?;
        Ok(session.as_ref().map(SessionPrivacy::of))
    }

    /// The roles of the viewer's user, which decide what they are cleared
    /// for (issue 133). Without them, only what is public: a failed lookup
    /// shows less.
    async fn roles(&self) -> Vec<String> {
        caelumora_model::confidentiality::user_roles(self.entities.as_ref(), self.viewer.context().initiator_id).await.unwrap_or_default()
    }
}

/// Where an edge leads (issue 164): to what is there, to what was there and
/// was deleted - by whom and when - or, as far as the viewer may know, to
/// nothing that ever was.
#[derive(Debug)]
pub enum Reached<T> {
    There(T),
    Gone(Removal),
    Never,
}

impl<T> Reached<T> {
    /// What is there, if it is.
    pub fn there(self) -> Option<T> {
        match self {
            Reached::There(value) => Some(value),
            Reached::Gone(_) | Reached::Never => None,
        }
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
    Session(Arc<SessionLoop>),
}

impl ViewsFactory {
    pub(crate) fn of_actor(runtime: ActorRuntime, context: SecurityContext) -> Self {
        Self { root: FactoryRoot::Actor(runtime), context }
    }

    #[cfg(test)]
    pub(crate) fn in_session(session: Arc<SessionLoop>, context: SecurityContext) -> Self {
        Self { root: FactoryRoot::Session(session), context }
    }

    /// The agent's own session, as it reads from `channel` at
    /// `classification`; `None` while the channel is not attached to its
    /// session yet, or once it is gone.
    pub(crate) fn session(&self, channel: ChannelId, classification: Classification) -> Option<SessionView> {
        let session_loop = match &self.root {
            FactoryRoot::Actor(actor) => actor.channel_loop()?.session_loop()?,
            #[cfg(test)]
            FactoryRoot::Session(session) => Arc::clone(session),
        };
        let from = Place { channel, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(self.context.clone(), from));
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
    use caelumora_model::snapshot::Detail;
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
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        (store, Arc::new(SessionManager::new(runtime)))
    }

    fn user(name: &str) -> User {
        User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None }
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
            deleted: None,
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
            deleted: None,
            former: Vec::new(),
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
            deleted: None,
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Helper", Scope::GlobalScope).unwrap(), &personality, owner);
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_context_of_an_external_agent_is_what_the_remote_agent_was_given() {
        use caelumora_model::agent::PersonalityKind;
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::message::{Message, MessageId};
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
        let mut state = AgentChannelState::default();
        // One call: the question was given, the transcript started anew,
        // and the answer came of it.
        state.transcript.extend(vec![question.clone()]);
        let given = caelumora_model::context::ContextRef { transcript: state.transcript.start_anew(), ..Default::default() };
        let later = said("later");
        state.transcript.extend(vec![later.clone()]);
        state.history.extend(vec![question.clone()]);
        state.history.push(answer.clone(), Some(given));
        state.history.extend(vec![later]);
        store.put_agent_channel_state(remote.id, channel.id, &mut state).await.unwrap();

        let for_alice = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let member = for_alice.member(ActorId::Agent(remote.id)).await.unwrap();
        let seen = member.context().await.expect("alice made the agent");
        let texts = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        assert_eq!(texts(seen.transcript().messages()), ["later"], "what it is given next, not what it was given or answered");
        assert_eq!(seen.private_notes(), (Vec::new(), None), "an external agent writes none");
        assert!(seen.given_up_turns().is_empty());
        let context = seen.context_of(answer.id).expect("what the call was given (issue 205)");
        assert_eq!(context.items.len(), 1, "the messages of the one call, no prompt parts");
        assert!(matches!(&context.items[0], caelumora_model::context::ContextItem::Conversation { message } if message.id == question.id));
        assert!(context.tool_sets.is_empty(), "and no tools: those are the remote agent's");
        assert_eq!(seen.classification_of(answer.id), Some(Classification::CHANNEL_DEFAULT), "its answer is in its history");
        assert_eq!(member.private_notes().await.unwrap(), (Vec::new(), None));
        assert!(member.notes().await.unwrap().expect("an agent").notes.is_empty(), "it writes none");
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
                assert_eq!(for_alice.snapshot(Detail::empty()).await.participants.iter().filter_map(|p| p.name.as_deref()).collect::<Vec<_>>(), ["alice", "bob", "Helper"], "{state}");
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
                let all = Detail::all();
                assert_eq!(listed[0].snapshot(all).await, for_bob.snapshot(all).await, "{state}: listed as it is reached");
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
        let alice = User { id: UserId::new(), username: "Alice Example".to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut session = make_session("s", ActorId::User(alice.id), vec![]);
        session.participants = record_users(&store, &[&alice]).await;
        let mut channel = main_channel(&session);
        channel.participants.extend([Participant::User(gone), Participant::Approver(approver)]);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap().snapshot(Detail::MEMBERS).await;
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
        store.put_channel_state(channel.id, &mut state).await.unwrap();
        // What the agent noted and saw.
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        agent_state.notes.push(Change::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.private_notes.push(Change::Add { id: NoteId(1), text: "mine".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.transcript.push(first.clone());
        store.put_agent_channel_state(helper.id, channel.id, &mut agent_state).await.unwrap();

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
    async fn an_agent_whose_personality_is_gone_keeps_its_name_and_its_state() {
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        // What it kept while it ran: the same state, whatever its kind was
        // (issue 206).
        let mut kept = caelumora_model::agent::AgentChannelState::default();
        kept.notes.push(Change::Add { id: NoteId(1), text: "kept".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(helper.id, channel.id, &mut kept).await.unwrap();
        assert!(store.delete_personality(&system, helper.personality_id).await.unwrap());

        let for_alice = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let member = for_alice.member(ActorId::Agent(helper.id)).await.unwrap();
        assert_eq!((member.name(), member.handle().as_deref()), (Some("Helper"), Some("helper")));
        assert!(member.may_read_context().await, "a question of its document, not of its kind");
        assert_eq!(member.introduction().await.unwrap(), None, "its personality introduced it");
        let notes = member.notes().await.unwrap().expect("still an agent");
        assert_eq!(notes.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["kept"], "what it kept is read");
        assert_eq!(for_alice.notes().await.unwrap().len(), 1, "and counts among the channel's notes");
        assert!(member.context().await.is_ok(), "as what it saw");
    }

    #[tokio::test]
    async fn the_approvals_a_channel_waits_for_are_read_from_its_agents_turns() {
        use caelumora_model::actor::ActorState;
        use caelumora_model::agent::AgentChannelState;
use caelumora_model::turn::TurnState;
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
        turn.work.event_context.session_id = session.id;
        turn.work.event_context.channel_id = channel.id;
        turn.work.event_context.sender_id = ActorId::Agent(helper.id);
        turn.tool_calls = vec![waiting.clone(), ToolCallState::approved(call("echo"))];
        let mut state = AgentChannelState { processing_state: ActorState::WaitingForApproval, turn: Some(turn), ..Default::default() };
        store.put_agent_channel_state(helper.id, channel.id, &mut state).await.unwrap();

        let main = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let pending = main.pending_approvals().await.unwrap();
        assert_eq!(pending.len(), 1, "only the call that waits");
        assert_eq!((pending[0].id, pending[0].agent_id, pending[0].channel_id, pending[0].description.as_str()), (waiting.id, helper.id, channel.id, "Delete it?"));
        assert!(manager.running(session.id).is_none(), "read without starting the session");
    }

    #[tokio::test]
    async fn a_snapshot_holds_the_details_asked_for_and_no_others() {
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId, TokenUsage};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::snapshot::MemberSnapshot;
        use caelumora_model::store::StateStore;
        use caelumora_model::tool::{ToolCall, ToolCallState};
        use caelumora_model::turn::TurnState;

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();
        // What the agent's call used, what it noted, and a call its turn
        // waits to have approved.
        let answer = Message {
            sender_id: ActorId::Agent(helper.id),
            prompt_tokens: Some(1000),
            completion_tokens: Some(50),
            context_size: Some(8192),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let mut state = AgentChannelState::default();
        state.transcript.push(answer);
        state.notes.push(Change::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        let mut turn = TurnState::default();
        turn.work.event_context.sender_id = ActorId::Agent(helper.id);
        let call = ToolCall { id: Some("delete".to_string()), function_name: "delete".to_string(), arguments: serde_json::json!({}), visibility: Default::default() };
        turn.tool_calls = vec![ToolCallState::pending(call, "Delete it?".to_string())];
        state.turn = Some(turn);
        store.put_agent_channel_state(helper.id, channel.id, &mut state).await.unwrap();

        let views = manager.views(as_user(alice.id));
        let main = views.session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let expected = TokenUsage { calls: 1, prompt_tokens: 1000, completion_tokens: 50, last_call_tokens: 1050, context_size: Some(8192) };
        let details = |member: &MemberSnapshot| (member.usage, member.notes.len(), member.pending_approvals.len());

        let all = main.snapshot(Detail::MEMBERS | Detail::USAGE | Detail::NOTES | Detail::APPROVALS).await;
        let [user, agent] = all.members.as_slice() else { panic!("two members: {all:?}") };
        assert_eq!(details(user), (None, 0, 0), "a user has none of them");
        assert_eq!(details(agent), (Some(expected), 1, 1), "an agent's, from its state");
        let alone = main.member(ActorId::Agent(helper.id)).await.unwrap().snapshot(Detail::USAGE).await;
        assert_eq!(details(&alone), (Some(expected), 0, 0), "only what was asked for");
        let plain = main.snapshot(Detail::MEMBERS).await;
        assert!(plain.members.iter().all(|m| details(m) == (None, 0, 0)), "none asked for");
        assert!(main.snapshot(Detail::USAGE).await.members.is_empty(), "no members to carry it");
        let listed = views.session(session.id).await.unwrap().snapshot(Detail::CHANNELS).await;
        assert!(!listed.channels.is_empty() && listed.channels.iter().all(|c| c.members.is_empty()), "only what was asked for");
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
        store.put_channel_state(secret.id, &mut state).await.unwrap();
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        let note = |id, text: &str, classification: &Classification| Change::Add { id: NoteId(id), text: text.to_string(), classification: classification.clone() };
        agent_state.notes.push(note(1, "noted secretly", &secret.classification), NoteOrigin::default(), MessageId::default());
        agent_state.notes.push(note(2, "noted openly", &public.classification), NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(helper.id, secret.id, &mut agent_state).await.unwrap();
        let mut state = ChannelState::new(team.id);
        state.history.extend(vec![said("internal", &team.classification)]);
        store.put_channel_state(team.id, &mut state).await.unwrap();

        // The agent, reading from one of its channels, as its components do.
        let reading_from = |channel: &Channel| {
            let context = SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(ActorId::Agent(helper.id));
            let from = Place { channel: channel.id, classification: channel.classification.clone(), session: SessionPrivacy::of(&session) };
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
    async fn a_member_that_left_is_still_reached_by_what_it_said_and_a_gone_channel_is_told_from_one_never_there() {
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
        store.put_channel_state(channel.id, &mut state).await.unwrap();

        let views = manager.views(as_user(alice.id));
        let reached = views.channel_of(session.id, channel.id).await.unwrap().there().expect("there");
        let former = reached.member(ActorId::Agent(helper.id)).await.expect("known by what it said");
        assert!(former.former() && former.name() == Some("Helper"));
        assert!(former.context().await.is_ok(), "what it saw is still the owner's to read");
        assert!(reached.members().await.iter().all(|m| m.id() != ActorId::Agent(helper.id)), "not listed among the members");
        assert!(matches!(reached.member(ActorId::User(UserId::new())).await, Err(ViewError::MemberNotFound { .. })), "a user who never took part");

        // Never there: another channel, another session - nor for a stranger.
        assert!(matches!(views.channel_of(session.id, ChannelId::new()).await.unwrap(), Reached::Never));
        assert!(matches!(views.channel_of(SessionId::new(), channel.id).await.unwrap(), Reached::Never));
        let stranger = manager.views(as_user(UserId::new()));
        assert!(matches!(stranger.channel_of(session.id, reached.id()).await.unwrap(), Reached::Never), "nor is it there for a stranger");

        // Gone (issue 164): the channel deleted, then the session - with who
        // deleted it, to whoever could read it, and never there for others.
        let alice_ctx = as_user(alice.id).context().clone();
        let mut deleted = channel.clone();
        deleted.id = ChannelId::new();
        store.create_channel(&system, &deleted).await.unwrap();
        store.delete_channel(&alice_ctx, deleted.id).await.unwrap();
        match views.channel_of(session.id, deleted.id).await.unwrap() {
            Reached::Gone(removal) => assert_eq!(removal.by, ActorId::User(alice.id)),
            other => panic!("gone, not {other:?}"),
        }
        assert!(matches!(stranger.channel_of(session.id, deleted.id).await.unwrap(), Reached::Never), "not to a stranger");
        store.delete_session(&alice_ctx, session.id).await.unwrap();
        assert!(matches!(views.channel_of(session.id, channel.id).await.unwrap(), Reached::Gone(_)), "gone with its session");
        assert!(matches!(stranger.channel_of(session.id, channel.id).await.unwrap(), Reached::Never));
    }

    #[tokio::test]
    async fn the_trash_shows_what_its_owner_deleted_with_what_they_may_do_about_it() {
        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let session = make_session("hers", ActorId::User(alice.id), vec![ActorId::User(bob.id)]);
        let channel = main_channel(&session);
        let mut side = channel.clone();
        side.id = ChannelId::new();
        side.name = caelumora_model::name::Name::new("side").unwrap();
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        store.create_channel(&system, &side).await.unwrap();
        let alice_ctx = as_user(alice.id).context().clone();
        store.delete_channel(&alice_ctx, side.id).await.unwrap();

        let hers = manager.views(as_user(alice.id));
        let channels = hers.session(session.id).await.unwrap().deleted_channels().await.unwrap();
        assert_eq!(channels.iter().map(|c| (c.id, c.name.as_str())).collect::<Vec<_>>(), [(side.id, "side")]);
        assert!(channels[0].can_restore && !channels[0].can_purge, "restored by her, purged only with the session");
        assert_eq!(channels[0].deleted.by, ActorId::User(alice.id));
        let his = manager.views(as_user(bob.id));
        assert!(his.session(session.id).await.unwrap().deleted_channels().await.unwrap().is_empty(), "not a contributor's trash");

        store.delete_session(&alice_ctx, session.id).await.unwrap();
        let sessions = hers.deleted_sessions().await.unwrap();
        assert_eq!(sessions.iter().map(|s| s.id).collect::<Vec<_>>(), [session.id]);
        assert!(sessions[0].can_restore && !sessions[0].can_purge, "purging is not the owner's");
        assert!(his.deleted_sessions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn who_left_is_a_former_member_with_who_removed_them_also_without_a_word() {
        use caelumora_model::channel::FormerParticipant;
        use caelumora_model::removal::Removal;

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![]);
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        // Both took part and were removed by alice, neither said anything.
        let mut channel = main_channel(&session);
        channel.participants.retain(|p| p.actor_id() != ActorId::User(bob.id) && p.actor_id() != ActorId::Agent(helper.id));
        let removed = Removal::now(ActorId::User(alice.id));
        channel.former = vec![
            FormerParticipant { participant: Participant::User(bob.id), left: removed.clone() },
            FormerParticipant { participant: Participant::Agent(helper.id), left: removed.clone() },
        ];
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();

        let reached = manager.views(as_user(alice.id)).channel_of(session.id, channel.id).await.unwrap().there().expect("there");
        let former = reached.member(ActorId::Agent(helper.id)).await.expect("kept as having left");
        assert!(former.former() && former.left() == Some(&removed));
        assert!(former.context().await.is_ok(), "what it saw is still the owner's to read");
        let bob_left = reached.member(ActorId::User(bob.id)).await.expect("a user too");
        assert!(bob_left.former() && bob_left.name() == Some("bob"));
        assert_eq!(reached.former_members().await.iter().map(MemberView::id).collect::<Vec<_>>(), [ActorId::User(bob.id), ActorId::Agent(helper.id)], "in the order they left");
        assert!(reached.members().await.iter().all(|m| !m.former()), "not among the members");

        let snapshot = reached.snapshot(Detail::MEMBERS).await;
        assert_eq!(snapshot.former_members.len(), 2);
        assert!(snapshot.former_members.iter().all(|m| m.left.as_ref() == Some(&removed)), "each with who removed it, and when");
        assert!(snapshot.members.iter().all(|m| m.left.is_none()));
    }
}
