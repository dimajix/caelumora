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
//! their classifications (`ConfidentialityQuestions::may_copy_information`, issue 133),
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
mod group;
mod member;
mod session;
mod workspace;

pub use channel::{ChannelView, MemberView};
pub use context::{AgentContextView, AgentContextError};
pub use group::GroupView;
pub use session::{PrivateNotesElsewhere, SessionView};
pub use workspace::WorkspaceView;

use caelumora_model::store::query::{Filter, Query};
use crate::runtime::{ActorRuntime, RuntimeManager, Services};
use crate::session::{Found, SessionLoop, SessionManager};
use crate::tool::ToolError;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions, Place};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::removal::Removal;
use caelumora_model::security::{AccessQuestions, AclEntry, Refused, SecurityAttributes, SecurityContext, SecurityManager, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::{AccessSnapshot, AgentSnapshot, DeletedSnapshot};
use caelumora_model::workspace::WorkspaceId;
use caelumora_model::group::{Group, GroupField, GroupId};
use caelumora_model::scope::Scope;
use crate::workspace::WorkspaceManager;
use caelumora_model::store::{EntityStores, StateStore, StoreError, StoreResult, StoreResultExt};
use crate::component::ComponentRegistry;
use std::sync::Arc;
use caelumora_model::session::Session;
use caelumora_model::workspace::Workspace;
use caelumora_model::removal::Stored;

/// Who navigates: in whose name every edge asks its question (issue 131),
/// and - for an agent - from where.
#[derive(Debug, Clone)]
pub struct Viewer {
    context: SecurityContext,
    /// Where an agent reads from: its channel, the classification it reads
    /// at (its turn's, issue 67) and its session's privacy (issue 134) -
    /// what every `may_copy_information` it asks copies into.
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
    #[error("There is no workspace {0}")]
    WorkspaceNotFound(WorkspaceId),
    #[error("There is no group {0}")]
    GroupNotFound(GroupId),
    /// Asked for by name, as an agent's tools do: no such channel, or
    /// nothing in it that can be read from where the viewer reads.
    #[error("there is no channel named \"{0}\" that can be read from here")]
    NoChannelNamed(String),
    #[error("Channel {channel} has no member {actor}")]
    MemberNotFound { channel: ChannelId, actor: ActorId },
    /// Reached, but not the viewer's to read: what an agent saw, which
    /// tools it has.
    #[error("You are not permitted to read this of {actor}")]
    Forbidden { actor: ActorId, #[source] refused: Refused },
    /// Not the viewer's to do: searching whom to take into a session they
    /// may not change (issue 221).
    #[error("You are not permitted to do this")]
    NotPermitted(#[source] Refused),
    /// Only agents have a context.
    #[error("There is nothing {actor} saw: it is no agent, or no longer exists")]
    NoContext { actor: ActorId },
    /// Reaching it failed; the cause is in the log.
    #[error("Reading failed")]
    Failed(#[source] anyhow::Error),
}

impl ViewError {
    /// Reaching something failed: `error`, with what was being done, for
    /// the log - the viewer learns only that it failed.
    pub(crate) fn failed<E>(error: E, doing: impl std::fmt::Display + Send + Sync + 'static) -> Self
    where E: std::error::Error + Send + Sync + 'static {
        ViewError::Failed(anyhow::Error::new(error).context(doing))
    }
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
/// A plugin's views are an extension trait on it (issue 323), built from
/// what it opens: the viewer's `context`, the `security` and
/// `confidentiality` policies, the runtime's `service`s - among them the
/// plugin's manager - and `channel_reading`.
#[derive(Clone)]
pub struct Views {
    services: Services,
    security: Arc<dyn SecurityManager>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    stores: EntityStores,
    states: Arc<dyn StateStore>,
    components: Arc<ComponentRegistry>,
    workspaces: Arc<WorkspaceManager>,
    running: Arc<dyn RunningSessions>,
    viewer: Viewer,
}

impl Views {
    pub(crate) fn new(runtime: &RuntimeManager, running: Arc<dyn RunningSessions>, viewer: Viewer) -> Self {
        Self {
            services: runtime.services().clone(),
            security: Arc::clone(runtime.security_manager()),
            confidentiality: Arc::clone(runtime.confidentiality()),
            stores: runtime.entity_stores().clone(),
            states: Arc::clone(runtime.state_store()),
            components: Arc::clone(runtime.component_registry()),
            workspaces: Arc::clone(runtime.workspace_manager()),
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
    pub fn context(&self) -> &SecurityContext {
        self.viewer.context()
    }

    /// The policy every edge asks, in the viewer's `context`.
    pub fn security(&self) -> &Arc<dyn SecurityManager> {
        &self.security
    }

    /// What may flow where, and who is cleared for what (issue 304).
    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> {
        &self.confidentiality
    }

    /// The service `T` of the runtime (`Services`) - a plugin's manager,
    /// which its views read through.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.services.get::<T>()
    }

    /// The stores of the entities, one per kind (issue 323).
    pub(crate) fn stores(&self) -> &EntityStores {
        &self.stores
    }

    pub(crate) fn states(&self) -> &Arc<dyn StateStore> {
        &self.states
    }

    pub(crate) fn components(&self) -> &ComponentRegistry {
        &self.components
    }

    pub(crate) fn workspace_manager(&self) -> &Arc<WorkspaceManager> {
        &self.workspaces
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        let sessions = self.stores.sessions.list(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the sessions"))?;
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
            None => Found::Stored(self.stores.sessions.get(self.viewer.context(), id).await.map_err(|e| {
                if matches!(e, StoreError::NotFound { .. } | StoreError::PermissionDenied(_)) { not_found() } else { ViewError::Failed(e.into()) }
            })?),
        };
        Ok(SessionView::of(self.clone(), found))
    }

    /// The channel `channel` of session `session`, wherever it is - what a
    /// memory entry's origin or a `KnowledgeRef` points to (issue 164):
    /// there, if the viewer may read it (asked as `session(id)` and
    /// `SessionView::channel` ask); gone, if it or its session was deleted
    /// and the viewer could read it were it there (`GetSession` on its
    /// stored attributes; a channel reached as a live one is, issue 304),
    /// with who deleted it and when; never there otherwise - so nothing can
    /// be probed.
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
            Err(ViewError::ChannelNotFound { .. }) => self.gone_channel(&session_view, channel).await,
            Err(e) => Err(e),
        }
    }

    /// Whether session `id` was deleted and the viewer could read it were
    /// it there.
    async fn gone_session<T>(&self, id: SessionId) -> ViewResult<Reached<T>> {
        let deleted = self.stores.sessions.get_deleted(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::failed(e, format!("looking up session {id}")))?;
        Ok(match deleted {
            Some(session) if self.security.may_read_session(self.viewer.context(), &session.security_attributes).await =>
                session.deleted.map_or(Reached::Never, Reached::Gone),
            _ => Reached::Never,
        })
    }

    /// Whether channel `id` of `session` was deleted and the viewer could
    /// read it were it there: reached as a live channel is
    /// (`SessionView::readable`, issue 304) - one the viewer is not cleared
    /// for is never there, as a live one would be not found.
    async fn gone_channel<T>(&self, session: &SessionView, id: ChannelId) -> ViewResult<Reached<T>> {
        let deleted = self.stores.channels.get_deleted(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::failed(e, format!("looking up channel {id}")))?;
        Ok(match deleted {
            Some(channel) if channel.session_id == session.id() && session.readable(&channel).await.is_some() =>
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
        let agents = self.stores.agents.list(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the agents"))?;
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
        let agents = self.stores.agents.list_deleted(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the deleted agents"))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(agents.len());
        for agent in agents {
            let Some(removal) = agent.deleted.clone() else { continue };
            let attributes = &agent.security_attributes;
            deleted.push(DeletedSnapshot {
                id: agent.id,
                name: agent.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), Agent::KIND, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), Agent::KIND, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// The workspaces the store lists for the viewer (issues 23, 322) -
    /// their own and those shared with them, wherever they lie, that they
    /// are cleared for -, in the order it lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn workspaces(&self) -> ViewResult<Vec<WorkspaceView>> {
        let workspaces = self.stores.workspaces.list(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the workspaces"))?;
        Ok(workspaces.into_iter().map(|workspace| WorkspaceView::of(self.clone(), workspace)).collect())
    }

    /// Workspace `id`, if the store lets the viewer reach it - read it
    /// (`GetWorkspace`, issue 23) and be cleared for it (issue 322).
    ///
    /// # Errors
    /// `WorkspaceNotFound` if there is none they may reach.
    pub async fn workspace(&self, id: WorkspaceId) -> ViewResult<WorkspaceView> {
        match self.stores.workspaces.get(self.viewer.context(), id).await.optional() {
            Ok(Some(workspace)) => Ok(WorkspaceView::of(self.clone(), workspace)),
            Ok(None) => Err(ViewError::WorkspaceNotFound(id)),
            Err(e) => Err(ViewError::failed(e, "reading a workspace")),
        }
    }

    /// Whom an entry of an access list may name the viewer by (issues 293,
    /// 308, 221): themselves, everyone if they are a user, the roles they
    /// hold and the teams they belong to - not those they only own -, as the
    /// policy resolves an entry. What a kind lists as shared with them by
    /// name asks the store for these.
    ///
    /// # Errors
    /// What the store reports.
    pub async fn subjects(&self) -> StoreResult<Vec<Subject>> {
        let context = self.viewer.context();
        let principal = context.principal_id;
        let roles = context.roles.iter().copied().filter(|role| context.has_role(*role));
        let teams = self.teams().await?.into_iter().filter(|team| team.has_member(principal)).map(|team| Subject::Group(team.id));
        // Every user is named by an entry for everyone (issue 221).
        let everyone = matches!(principal, ActorId::User(_)).then_some(Subject::Everyone);
        Ok(std::iter::once(Subject::Actor(principal)).chain(everyone).chain(roles.map(Subject::Role)).chain(teams).collect())
    }

    /// The entries `acl` with whom each names called by name (issue 23),
    /// read in the runtime's name: shown to the owner of what they are of,
    /// who may name a session or user they cannot read themselves. A group
    /// is called by its name - the group of the participants of a session or
    /// channel by the session's or channel's -, and says where it lies.
    pub async fn access_snapshots(&self, acl: &[AclEntry]) -> Vec<AccessSnapshot> {
        let system = SecurityContext::system();
        let mut access = Vec::with_capacity(acl.len());
        for entry in acl {
            let (name, scope) = match entry.subject {
                Subject::Actor(ActorId::User(user)) => (self.stores.users.get(&system, user).await.optional().ok().flatten().map(|u| u.username.to_string()), None),
                Subject::Actor(_) | Subject::Everyone => (None, None),
                Subject::Role(role) => (self.confidentiality.roles().and_then(|roles| roles.name_of(role)).map(str::to_string), None),
                Subject::Group(group) => match self.stores.groups.get(&system, group).await.optional().ok().flatten() {
                    Some(group) => (self.group_name(&group).await, Some(group.name.scope)),
                    None => (None, None),
                },
            };
            access.push(AccessSnapshot { subject: entry.subject, grant: entry.grant, name, scope });
        }
        access
    }

    /// How `group` is called: a team by its name, the participants of a
    /// session or a channel by the session's or channel's, read in the
    /// runtime's name.
    async fn group_name(&self, group: &Group) -> Option<String> {
        let system = SecurityContext::system();
        match group.name.scope {
            Scope::SessionScope { session_id } => self.stores.sessions.get(&system, session_id).await.optional().ok().flatten().map(|s| s.name.to_string()),
            Scope::ChannelScope { channel_id, .. } => self.stores.channels.get(&system, channel_id).await.optional().ok().flatten().map(|c| c.name.to_string()),
            _ => Some(group.name.to_string()),
        }
    }

    /// The teams the viewer may read (`GetGroup`, issue 286): those they own
    /// and those they belong to. The participants of a session or channel
    /// are reached through it, not listed here.
    ///
    /// # Errors
    /// `Failed` if the store fails.
    pub async fn groups(&self) -> ViewResult<Vec<GroupView>> {
        let teams = self.teams().await.map_err(|e| ViewError::failed(e, "listing the groups"))?;
        Ok(teams.into_iter().map(|team| GroupView::of(self.clone(), team)).collect())
    }

    /// The teams the viewer owns or belongs to (issue 317), which the store
    /// is asked for alone, in their name: the teams of `groups`, and those
    /// `subjects` names them by.
    async fn teams(&self) -> StoreResult<Vec<Group>> {
        let principal = self.viewer.context().principal_id;
        let theirs = Filter::Or(vec![
            Filter::eq(GroupField::Owner, principal),
            Filter::any(GroupField::Members, Filter::eq(GroupField::Member, principal)),
        ]);
        let query = Query::filtered(Filter::And(vec![Group::teams(), theirs]));
        Ok(self.stores.groups.list(self.viewer.context(), &query).await?.items)
    }

    /// Group `id`, if the viewer may read it (`GetGroup`) - a team, or the
    /// participants of a session or channel.
    ///
    /// # Errors
    /// `GroupNotFound` if there is none they may read, alike.
    pub async fn group(&self, id: GroupId) -> ViewResult<GroupView> {
        match self.stores.groups.get(self.viewer.context(), id).await.optional() {
            Ok(Some(group)) => Ok(GroupView::of(self.clone(), group)),
            Ok(None) => Err(ViewError::GroupNotFound(id)),
            Err(e) => Err(ViewError::failed(e, "reading a group")),
        }
    }

    /// The teams the viewer deleted and may read as such: the trash of
    /// groups. The participants of a session or channel are deleted and
    /// restored with it.
    ///
    /// # Errors
    /// `Failed` if the store fails.
    pub async fn deleted_groups(&self) -> ViewResult<Vec<DeletedSnapshot<GroupId>>> {
        let groups = self.stores.groups.list_deleted(self.viewer.context(), &Query::filtered(Group::teams())).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the deleted groups"))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(groups.len());
        for group in groups {
            let Some(removal) = group.deleted.clone() else { continue };
            let attributes = &group.security_attributes;
            deleted.push(DeletedSnapshot {
                id: group.id,
                name: group.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), Group::KIND, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), Group::KIND, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// The workspaces the viewer deleted and may read as such (issue 253):
    /// the trash of workspaces.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_workspaces(&self) -> ViewResult<Vec<DeletedSnapshot<WorkspaceId>>> {
        let workspaces = self.stores.workspaces.list_deleted(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the deleted workspaces"))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            let Some(removal) = workspace.deleted.clone() else { continue };
            let attributes = &workspace.security_attributes;
            deleted.push(DeletedSnapshot {
                id: workspace.id,
                name: workspace.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), Workspace::KIND, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), Workspace::KIND, attributes).await,
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
        let sessions = self.stores.sessions.list_deleted(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the deleted sessions"))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(sessions.len());
        for session in sessions {
            let Some(removal) = session.deleted.clone() else { continue };
            let attributes = &session.security_attributes;
            deleted.push(DeletedSnapshot {
                id: session.id,
                name: session.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), Session::KIND, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), Session::KIND, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// Whether what `from` holds may be copied into the viewer's place
    /// (`ConfidentialityQuestions::may_copy_information`, issues 133, 264,
    /// 304): an agent reading entry by entry. A viewer without a place - a
    /// user - reads whole channels by their clearance and nothing entry by
    /// entry, so nothing may be copied for them so.
    pub(crate) fn may_copy_from(&self, from: &Place) -> bool {
        let Some(into) = &self.viewer.from else { return false };
        self.confidentiality.may_copy_information(self.viewer.context(), from, into)
    }

    /// The name of `actor`; `None` for a user or agent that no longer
    /// exists. Read in the runtime's name: taking part in a session makes a
    /// name known to everybody in it - it stands at every message - even
    /// where the record itself is not theirs to read (issue 120); so does
    /// owning an agent there, which acts with its owner's authority (issue
    /// 244).
    async fn name_of(&self, actor: ActorId) -> Option<String> {
        match actor {
            ActorId::User(id) => self.stores.users.get(&SecurityContext::system(), id).await.ok().map(|user| user.username.to_string()),
            ActorId::Agent(id) => match self.agent_of(id).await {
                Some(agent) => Some(agent.name.to_string()),
                None => self.deleted_agent_name(id).await,
            },
            other => other.fixed_name().map(str::to_string),
        }
    }

    /// The name of agent `id` if it was deleted and not purged (issue 164):
    /// what it said keeps its sender. Read in the runtime's name, like its
    /// name while it exists.
    pub(super) async fn deleted_agent_name(&self, id: AgentId) -> Option<String> {
        self.stores.agents.get_deleted(&SecurityContext::system(), id).await.ok().map(|agent| agent.name.to_string())
    }

    /// Who deleted agent `id` and when, if it was deleted and not purged
    /// (issue 314): when it left the channels it rests in. Read in the
    /// runtime's name, like its name.
    pub(super) async fn agent_removal(&self, id: AgentId) -> Option<Removal> {
        self.stores.agents.get_deleted(&SecurityContext::system(), id).await.ok().and_then(|agent| agent.deleted)
    }

    /// The document of agent `id` - its name, its attributes, its
    /// personality - or `None` if it no longer exists. Read in the
    /// runtime's name, like its name.
    async fn agent_of(&self, id: AgentId) -> Option<Agent> {
        self.stores.agents.get(&SecurityContext::system(), id).await.ok()
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
        let session = self.stores.sessions.get(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::failed(e, format!("looking up session {id}")))?;
        Ok(session.as_ref().map(SessionPrivacy::of))
    }
}

/// How a channel may be read, where it may be read at all (issue 133): as
/// a whole, or only what was released to where the reader reads from,
/// entry by entry, each by its own classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelReading {
    Whole,
    Released,
}

/// How someone reads a channel (issue 133): an agent from its own place -
/// its channel, the classification it reads at, its session's privacy -,
/// a user by what the roles of their context clear them for.
#[derive(Debug, Clone, Copy)]
pub enum ChannelReader<'a> {
    From(&'a Place),
    Cleared,
}

/// How `context` may read the channel with `channel` attributes, which lies
/// `at` its place (issues 133, 264, 296, 304): not at all if it may not read
/// it (`GetChannel`, the `SecurityManager`'s). Beyond that, as the
/// `ConfidentialityPolicy` decides: an agent reading from its place reads it
/// as a whole if what is there may be copied into that place, and entry by
/// entry otherwise; a user reads it as a whole if they are cleared for its
/// classification, and not at all otherwise. What the views reach a channel
/// by, and what an operation in a channel asks before it acts there.
pub async fn channel_reading(security: &dyn SecurityManager, confidentiality: &dyn ConfidentialityPolicy, context: &SecurityContext, channel: &SecurityAttributes, at: &Place, reader: ChannelReader<'_>) -> Option<ChannelReading> {
    if !security.may_read_channel(context, channel).await {
        return None;
    }
    match reader {
        ChannelReader::From(from) => Some(match confidentiality.may_copy_information(context, at, from) {
            true => ChannelReading::Whole,
            false => ChannelReading::Released,
        }),
        ChannelReader::Cleared => confidentiality.cleared_for(context, &at.classification).then_some(ChannelReading::Whole),
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
/// the actor's runtime, which reaches its channel and session once they are
/// attached. Who reads, from which channel and at which classification is
/// the work's, given when the views are built - one context per piece of
/// work (issue 255).
#[derive(Clone)]
pub struct ViewsFactory {
    actor: ActorRuntime,
}

impl ViewsFactory {
    pub(crate) fn of_actor(actor: ActorRuntime) -> Self {
        Self { actor }
    }

    /// The views of an actor in a channel of no running session - for turns
    /// in tests that read nothing elsewhere: their `session` is `None`, as
    /// for a channel not attached to its session yet.
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        let runtime = crate::session::testing::test_runtime();
        let commands = Arc::new(crate::command::CommandExecutor::empty(Arc::clone(runtime.security_manager())));
        let session_id = SessionId::new();
        let channel = crate::runtime::ChannelRuntime::outside_a_session(runtime, commands, session_id);
        Self::of_actor(channel.new_actor_runtime(ActorId::None, session_id, ChannelId::new()))
    }

    /// The attributes of the agent's own channel as it stands, by which an
    /// act on what the agent keeps there is asked (`Reclassified::Note`,
    /// issue 312); `None` while the actor is not attached to a channel.
    pub(crate) fn channel_attributes(&self) -> Option<SecurityAttributes> {
        self.actor.channel_loop().map(|channel| channel.security_attributes())
    }

    /// The agent's own session, as it reads in `context` from `channel` at
    /// `classification`, if it may read it (`GetSession`, as `Views::session`
    /// asks, issue 304) - the agent by the group of the session's
    /// participants; `None` while the channel is not attached to its session
    /// yet, once it is gone, or if it may not.
    pub(crate) async fn session(&self, context: &SecurityContext, channel: ChannelId, classification: Classification) -> Option<SessionView> {
        let session_loop = self.actor.channel_loop()?.session_loop()?;
        let from = Place { channel, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(context.clone(), from));
        let attributes = session_loop.session().security_attributes.clone();
        if !views.security().may_read_session(context, &attributes).await {
            tracing::debug!(session = %session_loop.id(), caller = %context.caller_id, "An agent may not read its own session");
            return None;
        }
        Some(SessionView::of(views, Found::Running(session_loop)))
    }
}

impl std::fmt::Debug for ViewsFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewsFactory").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError};
    use crate::channel::ChannelLoop;
    use caelumora_model::actor::Rejection;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::snapshot::Detail;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::value::text::{Briefing, Summary};
    use crate::runtime::RuntimeManager;
    use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::{Classification, ConfidentialityRules, Visibility};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{Grant, SecurityAttributes, Subject};
    use crate::group::keep_participants;
    use caelumora_model::session::Session;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use caelumora_model::config::AppConfig;
    use crate::testing::test_registry;
    use caelumora_model::value::text::Introduction;

    fn as_user(id: UserId) -> Viewer {
        Viewer::new(SecurityContext::new(ActorId::User(id), Default::default(), Default::default()))
    }

    /// Stores `session` as the runtime does, with the group of its
    /// participants (issue 286).
    async fn stored_session(store: &Arc<MemoryStore>, session: &Session) {
        store.entity_stores().sessions.create(&SecurityContext::system(), session).await.unwrap();
        keep_participants(store.entity_stores().groups.as_ref(), Group::of_session(session)).await.unwrap();
    }

    /// Stores `channel` as the runtime does, with the group of its
    /// participants.
    async fn stored_channel(store: &Arc<MemoryStore>, channel: &Channel) {
        store.entity_stores().channels.create(&SecurityContext::system(), channel).await.unwrap();
        keep_participants(store.entity_stores().groups.as_ref(), Group::of_channel(channel)).await.unwrap();
    }

    /// Another channel like `template`, named `name`: an id of its own,
    /// read by its own members.
    fn like(template: &Channel, name: &str) -> Channel {
        let id = ChannelId::new();
        let security_attributes = SecurityAttributes::owned_by(template.security_attributes.owner_id).granting(Subject::Group(GroupId::of_channel(id)), Grant::Read);
        Channel { id, name: Name::new(name).unwrap(), security_attributes, ..template.clone() }
    }

    async fn an_agent_of(store: &Arc<MemoryStore>, owner: ActorId) -> Agent {
        an_agent_of_kind(store, owner, Default::default()).await
    }

    async fn an_agent_of_kind(store: &Arc<MemoryStore>, owner: ActorId, kind: PersonalityKind) -> Agent {
        let personality = Personality {
            description: None,
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
        store.entity_stores().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Helper", Scope::GlobalScope).unwrap(), &personality, owner);
        store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_context_of_an_external_agent_is_what_the_remote_agent_was_given() {
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let remote = an_agent_of_kind(&store, ActorId::User(alice.id), PersonalityKind::External).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(remote.id));
        let channel = main_channel(&session);
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

        let said = |t: &str| Message { id: MessageId::new(), content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (question, answer) = (said("question"), Message { turn_id: Some(caelumora_model::work::TurnId::new()), ..said("answer") });
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
        assert!(!seen.history().iter().any(|item| matches!(item, caelumora_model::message_history::HistoryEntry::TurnGivenUp { .. })));
        let context = seen.context_of(answer.id).expect("what the call was given (issue 205)");
        assert_eq!(context.items.len(), 1, "the messages of the one call, no prompt parts");
        assert!(matches!(&context.items[0], caelumora_model::context::ContextItem::Conversation { message } if message.id == question.id));
        assert!(context.tool_sets.is_empty(), "and no tools: those are the remote agent's");
        assert_eq!(seen.classification_of(answer.id), Some(Classification::CHANNEL_DEFAULT), "its answer is in its history");
        assert_eq!(member.private_notes().await.unwrap(), (Vec::new(), None));
        assert!(member.notes().await.unwrap().expect("an agent").notes.is_empty(), "it writes none");
        assert!(for_alice.notes().await.unwrap().iter().all(|published| published.notes.is_empty()));
        assert!(!member.may_retry().await, "it runs elsewhere and does not answer again (issue 242)");
        assert!(!member.snapshot(Detail::empty()).await.can_retry);
    }

    #[tokio::test]
    async fn the_views_show_what_their_viewer_may_read_and_do_whether_the_session_runs_or_not() {
        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;

        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let main = main_channel(&session);
        let running = manager.create_session(as_user(alice.id).context(), session, vec![main]).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = running.create_channel(as_user(alice.id).context(), crate::session::ChannelSpec { name: "private".to_string(), components: Default::default(), classification: None, language: None, uses_workspace: None, participants: vec![] }).await.unwrap().id();
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
                assert!(agent.may_cancel().await, "{state}: she may end its answer (issue 243)");
                assert_eq!(agent.state().is_some(), state == "running", "{state}: where it stands, only while it runs");
                let main = &for_bob.channels().await[0];
                assert!(!main.may_change().await, "{state}: a participant does not change it");
                for member in main.members().await {
                    assert!(!member.may_read_context().await, "{state}: bob reads no agent's context");
                    assert_eq!(member.may_retry().await, member.id() == ActorId::Agent(helper.id), "{state}: a participant asks an agent to answer again, nobody else");
                    assert_eq!(member.may_cancel().await, member.id() == ActorId::Agent(helper.id), "{state}: and ends its answer, nobody else's");
                    assert!(member.id() == ActorId::Agent(helper.id) || member.state().is_none(), "{state}: only an agent stands somewhere");
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

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("Alice Example");
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut session = make_session("s", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        let mut channel = main_channel(&session);
        channel.participants.extend([Participant::User(gone), Participant::Approver(approver)]);
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap().snapshot(Detail::MEMBERS).await;
        let [alice, gone, approver] = view.members.as_slice() else { panic!("three members, in order: {view:?}") };
        assert_eq!((alice.name.as_deref(), alice.handle.as_deref()), (Some("Alice Example"), Some("alice-example")));
        assert_eq!((gone.name.as_deref(), gone.handle.as_deref()), (None, None), "a deleted user is listed, but cannot be mentioned");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
        assert!(view.members.iter().all(|m| !m.can_read_context), "no member is an agent");
    }

    #[tokio::test]
    async fn what_a_place_is_for_is_shown_its_briefing_only_where_asked_for() {
        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        (session.summary, session.briefing) = (Some(Summary::new("Release 2.0").unwrap()), Some(Briefing::new("Dates are in UTC.").unwrap()));
        let mut channel = main_channel(&session);
        (channel.summary, channel.briefing) = (Some(Summary::new("Planning").unwrap()), Some(Briefing::new("Be brief.").unwrap()));
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap();
        let listed = view.snapshot(Detail::CHANNELS).await;
        assert_eq!((listed.summary.as_ref().map(Summary::as_str), listed.briefing.as_ref()), (Some("Release 2.0"), None), "the summary always, the briefing not where listed");
        assert_eq!((listed.channels[0].summary.as_ref().map(Summary::as_str), listed.channels[0].briefing.as_ref()), (Some("Planning"), None));
        let read = view.snapshot(Detail::CHANNELS | Detail::BRIEFING).await;
        assert_eq!((read.briefing.as_ref().map(Briefing::as_str), read.channels[0].briefing.as_ref().map(Briefing::as_str)), (Some("Dates are in UTC."), Some("Be brief.")));
    }

    #[tokio::test]
    async fn a_members_profile_says_who_it_is_to_everybody_in_the_channel() {
        use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
        use caelumora_model::user::UserProfile;

        let (store, manager) = owned_runtime(Default::default());
        let system = SecurityContext::system();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        // Carol owns the agent but takes no part in the session.
        store.entity_stores().users.create(&system, &carol).await.unwrap();
        let remote = an_agent_of_kind(&store, ActorId::User(carol.id), PersonalityKind::External).await;
        let mut personality = store.entity_stores().personalities.get(&system, remote.personality_id).await.unwrap();
        personality.introduction = Some(Introduction::new("Answers questions about the weather.").unwrap());
        store.entity_stores().personalities.update(&system, &personality).await.unwrap();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };

        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(remote.id));
        let mut channel = main_channel(&session);
        channel.participants.push(Participant::Approver(approver));
        store.entity_stores().user_profiles.create(&system, &UserProfile::new(alice.id, Some(Introduction::new("I run the project.").unwrap()))).await.unwrap();
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

        let for_bob = manager.views(as_user(bob.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let plain = for_bob.snapshot(Detail::MEMBERS).await;
        assert!(plain.members.iter().all(|m| m.introduction.is_none() && m.personality.is_none() && m.owner.is_none()), "only with PROFILE: {plain:?}");

        let view = for_bob.snapshot(Detail::MEMBERS | Detail::PROFILE).await;
        let [alice_seen, bob_seen, agent, approver_seen] = view.members.as_slice() else { panic!("four members, in order: {view:?}") };
        assert_eq!(alice_seen.introduction.as_deref(), Some("I run the project."), "as she introduces herself");
        assert_eq!((alice_seen.personality.as_deref(), alice_seen.owner), (None, None), "a user has neither personality nor owner");
        assert_eq!(bob_seen.introduction, None, "bob did not introduce himself");
        assert_eq!(agent.introduction.as_deref(), Some("Answers questions about the weather."), "as its personality introduces it");
        assert_eq!((agent.personality.as_deref(), agent.personality_kind), (Some("plain"), Some(PersonalityKind::External)), "public, though bob may not read the personality");
        assert_eq!((agent.owner, agent.owner_name.as_deref()), (Some(ActorId::User(carol.id)), Some("carol")), "named although carol is not in the session");
        assert_eq!((&approver_seen.introduction, &approver_seen.personality, approver_seen.owner), (&None, &None, None), "an approver is nobody to introduce");

        let alone = for_bob.member(ActorId::Agent(remote.id)).await.unwrap().snapshot(Detail::PROFILE).await;
        assert_eq!(&alone, agent, "a member asked for alone is shown as in the list");
    }

    #[tokio::test]
    async fn what_a_channel_holds_is_read_through_its_nodes_and_a_members_context_by_who_may() {
        use caelumora_model::channel::{ChannelState, HistoryLimit, HistoryStart};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let mut channel = main_channel(&session);
        stored_session(&store, &session).await;

        // What was said, of which bob - who joined later - sees the end only.
        let said = |t: &str| Message { content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (first, second) = (said("first"), said("second"));
        channel.history_limits.push(HistoryLimit { participant: ActorId::User(bob.id), start: HistoryStart::After(first.id) });
        // And so does the agent, though alice, for whom it acts, sees it all.
        channel.history_limits.push(HistoryLimit { participant: ActorId::Agent(helper.id), start: HistoryStart::After(first.id) });
        stored_channel(&store, &channel).await;
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
        assert_eq!(texts(&for_alice.history().await.unwrap().messages()), ["first", "second"]);
        assert_eq!(texts(&reach(as_user(bob.id)).await.history().await.unwrap().messages()), ["second"], "bob joined after the first");
        let as_helper = Viewer::new(SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(ActorId::Agent(helper.id), ActorId::User(alice.id)));
        assert_eq!(texts(&reach(as_helper).await.history().await.unwrap().messages()), ["second"], "the agent reads with its own limit, not its owner's");
        let notes = for_alice.notes().await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].agent, notes[0].name.as_str(), notes[0].notes[0].text.as_str()), (ActorId::Agent(helper.id), "Helper", "noted"));

        let agent = for_alice.member(ActorId::Agent(helper.id)).await.unwrap();
        let seen = agent.context().await.expect("alice made the agent");
        assert_eq!(seen.private_notes().0[0].text, "mine");
        assert_eq!(seen.transcript().messages().len(), 1);
        assert!(matches!(seen.context_of(first.id), Err(AgentContextError::NotRecorded { .. })), "the agent did not produce it");
        let refused = reach(as_user(bob.id)).await.member(ActorId::Agent(helper.id)).await.unwrap().context().await.expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let none = for_alice.member(ActorId::User(bob.id)).await.unwrap().context().await.expect_err("a user saw nothing");
        assert!(matches!(none, ViewError::NoContext { .. }), "{none}");
        assert!(matches!(for_alice.member(ActorId::System).await, Err(ViewError::MemberNotFound { .. })));
    }

    /// An agent reads another channel of its session only where its owner
    /// may read it or it takes part in it itself (issue 264): bob's agent,
    /// which left alice's channel "hers", does not reach it from "team" -
    /// though what it holds is classified as "team" is - but still knows
    /// what it wrote there itself, its private notes, through itself.
    #[tokio::test]
    async fn an_agent_does_not_reach_a_channel_its_owner_may_not_read_but_knows_what_it_wrote_there() {
        use caelumora_model::channel::{ChannelState, FormerParticipant};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::privacy::SessionPrivacy;
        use caelumora_model::removal::Removal;
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(bob.id)).await;
        let mut session = make_session("shared", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        let team = like(&template, "team");
        let hers = Channel {
            id: ChannelId::new(),
            name: Name::new("hers").unwrap(),
            participants: vec![Participant::User(alice.id)],
            security_attributes: SecurityAttributes::owned_by(ActorId::User(alice.id)),
            former: vec![FormerParticipant { participant: Participant::Agent(helper.id), left: Removal::now(ActorId::User(alice.id)) }],
            ..template.clone()
        };
        session.channels = vec![team.id, hers.id];
        stored_session(&store, &session).await;
        for channel in [&team, &hers] {
            stored_channel(&store, channel).await;
        }
        // Classified as team is: the flow rule alone would let team read it all.
        let mut state = ChannelState::new(hers.id);
        state.history.extend(vec![Message { id: MessageId::new(), content: Some("hers alone".to_string()), ..Message::new(hers.classification.clone()) }]);
        store.put_channel_state(hers.id, &mut state).await.unwrap();
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        let note = |text: &str| Change::Add { id: NoteId(1), text: text.to_string(), classification: hers.classification.clone() };
        agent_state.notes.push(note("noted for all"), NoteOrigin::default(), MessageId::default());
        agent_state.private_notes.push(note("noted for itself"), NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(helper.id, hers.id, &mut agent_state).await.unwrap();

        let context = SecurityContext::new(ActorId::User(bob.id), session.id, team.id).next(ActorId::Agent(helper.id), ActorId::User(bob.id));
        let from = Place { channel: team.id, classification: team.classification.clone(), session: SessionPrivacy::of(&session) };
        let from_team = manager.views(Viewer::from_place(context, from)).session(session.id).await.unwrap();
        let names: Vec<String> = from_team.channels().await.iter().map(|c| c.name().to_string()).collect();
        assert_eq!(names, ["team"], "not hers as a whole");
        assert!(matches!(from_team.channel_named("hers").await, Err(ViewError::NoChannelNamed(_))), "not reached at all: it answers as absent");
        assert!(matches!(from_team.channel(hers.id).await, Err(ViewError::ChannelNotFound { .. })), "also by its id");
        assert!(from_team.all_channels().await.iter().all(|c| c.id() != hers.id), "nor entry by entry");
        let own: Vec<_> = from_team.own_private_notes().await.into_iter().map(|found| (found.name, found.notes[0].text.clone())).collect();
        assert_eq!(own, [("hers".to_string(), "noted for itself".to_string())], "what it wrote there itself, by the flow rule alone");
    }

    #[tokio::test]
    async fn an_agents_private_notes_are_read_from_every_channel_of_the_session_by_who_may() {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let mut channel_of = |name: &str| {
            let channel = like(&template, name);
            session.channels.push(channel.id);
            channel
        };
        let (zeta, quiet, alpha) = (channel_of("zeta"), channel_of("quiet"), channel_of("alpha"));
        stored_session(&store, &session).await;
        for channel in [&zeta, &quiet, &alpha] {
            stored_channel(&store, channel).await;
        }
        for (channel, text) in [(&zeta, "noted in zeta"), (&alpha, "noted in alpha")] {
            let mut state = caelumora_model::agent::AgentChannelState::default();
            state.private_notes.push(Change::Add { id: NoteId(1), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
            store.put_agent_channel_state(helper.id, channel.id, &mut state).await.unwrap();
        }

        let read = |viewer: Viewer| {
            let manager = &manager;
            async move { manager.views(viewer).session(session.id).await.unwrap().private_notes_of(helper.id).await }
        };
        let found = read(as_user(alice.id)).await.expect("alice made the agent");
        let found: Vec<_> = found.iter().map(|(channel, notes, _)| (channel.name().as_str(), notes[0].text.as_str())).collect();
        assert_eq!(found, [("alpha", "noted in alpha"), ("zeta", "noted in zeta")], "by the channels' names, without one noting nothing");
        let refused = read(as_user(bob.id)).await.expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
    }

    #[tokio::test]
    async fn an_agent_whose_personality_is_gone_keeps_its_name_and_its_state() {
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let system = SecurityContext::system();
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;
        // What it kept while it ran: the same state, whatever its kind was
        // (issue 206).
        let mut kept = caelumora_model::agent::AgentChannelState::default();
        kept.notes.push(Change::Add { id: NoteId(1), text: "kept".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        store.put_agent_channel_state(helper.id, channel.id, &mut kept).await.unwrap();
        assert!(store.entity_stores().personalities.delete(&system, helper.personality_id).await.unwrap());

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

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

        // The agent's turn waits for one call, the other was approved.
        let call = |name: &str| ToolCall { id: Some(name.to_string()), function_name: name.to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let waiting = ToolCallState::pending(call("delete"), "Delete it?".to_string(), false, caelumora_model::approval::ToolMatcher::everything(), None);
        let mut turn = TurnState::default();
        turn.work.event_context.session_id = session.id;
        turn.work.event_context.channel_id = channel.id;
        turn.work.event_context.sender_id = ActorId::Agent(helper.id);
        turn.tool_calls = vec![waiting.clone(), ToolCallState::approved(call("echo"))];
        let mut state = AgentChannelState { processing_state: ActorState::WaitingForUser, turn: Some(turn), ..Default::default() };
        store.put_agent_channel_state(helper.id, channel.id, &mut state).await.unwrap();

        let main = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        let pending = main.pending_approvals().await.unwrap();
        assert_eq!(pending.len(), 1, "only the call that waits");
        assert_eq!((pending[0].id, pending[0].agent_id, pending[0].channel_id, pending[0].description.as_str()), (waiting.approval_id().unwrap(), helper.id, channel.id, "Delete it?"));
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

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;
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
        let call = ToolCall { id: Some("delete".to_string()), function_name: "delete".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        turn.tool_calls = vec![ToolCallState::pending(call, "Delete it?".to_string(), false, caelumora_model::approval::ToolMatcher::everything(), None)];
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
        let (store, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let mut channel_of = |name: &str, classification: Classification| {
            let channel = Channel { classification, ..like(&template, name) };
            session.channels.push(channel.id);
            channel
        };
        let public = channel_of("public", classified(Confidentiality::Public, &[]));
        let team = channel_of("team", classified(Confidentiality::Internal, &[]));
        let secret = channel_of("secret", classified(Confidentiality::Confidential, &[]));
        let hr = channel_of("hr", classified(Confidentiality::Internal, &["hr"]));
        stored_session(&store, &session).await;
        for channel in [&public, &team, &secret, &hr] {
            stored_channel(&store, channel).await;
        }

        // Said and noted in secret: one of each released to everyone.
        let said = |t: &str, classification: &Classification| Message { id: MessageId::new(), content: Some(t.to_string()), ..Message::new(classification.clone()) };
        let mut state = ChannelState::new(secret.id);
        state.history.extend(vec![said("the plan", &secret.classification), said("the date", &public.classification)]);
        let ended = caelumora_model::message_history::HistoryEntry::TurnGivenUp { turn_id: caelumora_model::work::TurnId::new(), actor: ActorId::Agent(helper.id), reason: caelumora_model::turn::GivenUpReason::Stopped, at: chrono::Utc::now() };
        state.history.end_turn(ended);
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
            let context = SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(ActorId::Agent(helper.id), ActorId::User(alice.id));
            let from = Place { channel: channel.id, classification: channel.classification.clone(), session: SessionPrivacy::of(&session) };
            Viewer::from_place(context, from)
        };
        let names = |channels: Vec<ChannelView>| channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>();
        let texts = |history: crate::channel::VisibleHistory| history.messages().iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();

        let from_team = manager.views(reading_from(&team)).session(session.id).await.unwrap();
        assert_eq!(names(from_team.channels().await), ["public", "team"], "as a whole: what is public, and itself");
        assert_eq!(names(from_team.all_channels().await), ["public", "team", "secret", "hr"], "the rest entry by entry");
        let secret_from_team = from_team.channel_named("secret").await.expect("reachable for what was released");
        let released = secret_from_team.history().await.unwrap();
        assert_eq!(released.items.len(), 1, "nothing of how turns ended: it would tell of work whose messages are not released (issue 258)");
        assert_eq!(texts(released), ["the date"]);
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
        let history = whole.history().await.unwrap();
        assert_eq!(history.items.len(), 3, "its messages, and how its turns ended (issue 258)");
        assert_eq!(texts(history), ["the plan", "the date"], "a channel reads itself as a whole");
        assert!(whole.member(ActorId::Agent(helper.id)).await.unwrap().context().await.is_ok());
    }

    #[tokio::test]
    async fn a_member_that_left_is_still_reached_by_what_it_said_and_a_gone_channel_is_told_from_one_never_there() {
        use caelumora_model::channel::ChannelState;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        // The agent took part, said something, and was removed.
        let channel = main_channel(&session);
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;
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
        deleted.name = Name::new("deleted").unwrap();
        stored_channel(&store, &deleted).await;
        store.entity_stores().channels.delete(&alice_ctx, deleted.id).await.unwrap();
        match views.channel_of(session.id, deleted.id).await.unwrap() {
            Reached::Gone(removal) => assert_eq!(removal.by, ActorId::User(alice.id)),
            other => panic!("gone, not {other:?}"),
        }
        assert!(matches!(stranger.channel_of(session.id, deleted.id).await.unwrap(), Reached::Never), "not to a stranger");
        store.entity_stores().sessions.delete(&alice_ctx, session.id).await.unwrap();
        assert!(matches!(views.channel_of(session.id, channel.id).await.unwrap(), Reached::Gone(_)), "gone with its session");
        assert!(matches!(stranger.channel_of(session.id, channel.id).await.unwrap(), Reached::Never));
    }

    #[tokio::test]
    async fn a_gone_channel_is_reached_as_a_live_one_by_the_clearance_too() {
        use caelumora_model::confidentiality::{Clearance, Confidentiality};
        use caelumora_model::role::{RoleId, Roles};
        let staff = RoleId::new();
        let roles = Roles::new([(staff, "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let (store, manager) = owned_runtime(ConfidentialityRules::default().with_roles(roles));
        let alice = user("alice");
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        let main = Channel { classification: Classification::general(Confidentiality::Internal), ..main_channel(&session) };
        let internal = Channel { id: ChannelId::new(), name: Name::new("side").unwrap(), ..main.clone() };
        let secret = Channel { id: ChannelId::new(), name: Name::new("secret").unwrap(), classification: Classification::general(Confidentiality::Confidential), ..main.clone() };
        stored_session(&store, &session).await;
        for channel in [&main, &internal, &secret] {
            stored_channel(&store, channel).await;
        }
        store.entity_stores().channels.delete(&SecurityContext::system(), internal.id).await.unwrap();
        store.entity_stores().channels.delete(&SecurityContext::system(), secret.id).await.unwrap();

        // Alice owns both, and is cleared for internal alone (issue 304).
        let hers = manager.views(Viewer::new(SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default()).with_roles(vec![staff], None)));
        assert!(matches!(hers.channel_of(session.id, internal.id).await.unwrap(), Reached::Gone(_)), "gone, where she could read it");
        assert!(matches!(hers.channel_of(session.id, secret.id).await.unwrap(), Reached::Never), "never there, as a live one above her clearance is not found");
    }

    #[tokio::test]
    async fn the_trash_shows_what_its_owner_deleted_with_what_they_may_do_about_it() {
        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let channel = main_channel(&session);
        let side = like(&channel, "side");
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;
        stored_channel(&store, &side).await;
        let alice_ctx = as_user(alice.id).context().clone();
        store.entity_stores().channels.delete(&alice_ctx, side.id).await.unwrap();

        let hers = manager.views(as_user(alice.id));
        let channels = hers.session(session.id).await.unwrap().deleted_channels().await.unwrap();
        assert_eq!(channels.iter().map(|c| (c.id, c.name.as_str())).collect::<Vec<_>>(), [(side.id, "side")]);
        assert!(channels[0].can_restore && !channels[0].can_purge, "restored by her, purged only with the session");
        assert_eq!(channels[0].deleted.by, ActorId::User(alice.id));
        let his = manager.views(as_user(bob.id));
        assert!(his.session(session.id).await.unwrap().deleted_channels().await.unwrap().is_empty(), "not a contributor's trash");

        store.entity_stores().sessions.delete(&alice_ctx, session.id).await.unwrap();
        let sessions = hers.deleted_sessions().await.unwrap();
        assert_eq!(sessions.iter().map(|s| s.id).collect::<Vec<_>>(), [session.id]);
        assert!(sessions[0].can_restore && !sessions[0].can_purge, "purging is not the owner's");
        assert!(his.deleted_sessions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn who_left_is_a_former_member_with_who_removed_them_also_without_a_word() {
        use caelumora_model::channel::FormerParticipant;
        use caelumora_model::removal::Removal;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        // Both took part and were removed by alice, neither said anything.
        let mut channel = main_channel(&session);
        channel.participants.retain(|p| p.actor_id() != ActorId::User(bob.id) && p.actor_id() != ActorId::Agent(helper.id));
        let removed = Removal::now(ActorId::User(alice.id));
        channel.former = vec![
            FormerParticipant { participant: Participant::User(bob.id), left: removed.clone() },
            FormerParticipant { participant: Participant::Agent(helper.id), left: removed.clone() },
        ];
        stored_session(&store, &session).await;
        stored_channel(&store, &channel).await;

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

    #[tokio::test]
    async fn a_channel_reads_another_by_their_classification_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let runtime = Arc::clone(runtime_of(&manager));

        let (alice, bob) = (user("alice"), user("bob"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        // Her context carries her roles, as her controller gives it (issue 304).
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default()).with_roles(alice.roles.clone(), None);
        let mut session = make_session("hers", alice_id);
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(bob_id), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification, participants| ChannelSpec { classification: Some(classification), ..spec(name, participants) };
        let public = session.create_channel(&alice_ctx, with("public", classified(Confidentiality::Public, &[]), vec![bob_id])).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]), vec![bob_id])).await.unwrap();
        let secret = session.create_channel(&alice_ctx, with("secret", classified(Confidentiality::Confidential, &[]), vec![])).await.unwrap();
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]), vec![])).await.unwrap();

        let confidentiality = runtime.confidentiality();
        let may_read = |reader: &Arc<ChannelLoop>, source: &Arc<ChannelLoop>| {
            let (reader, source, confidentiality, context) = (Arc::clone(reader), Arc::clone(source), Arc::clone(confidentiality), alice_ctx.clone());
            async move {
                use caelumora_model::confidentiality::{ConfidentialityQuestions, Place};
                let place = |channel: &ChannelLoop| Place { channel: channel.id(), classification: channel.classification(), session: channel.session_loop().map(|s| s.privacy()).unwrap_or_default() };
                confidentiality.may_copy_information(&context, &place(&source), &place(&reader))
            }
        };
        assert!(may_read(&secret, &secret).await, "a channel reads itself");
        assert!(may_read(&team, &public).await);
        assert!(!may_read(&public, &team).await, "a public channel reads only what is public");
        assert!(!may_read(&team, &secret).await, "confidential is not read from internal");
        assert!(may_read(&secret, &team).await, "although bob, who is in team, is not in secret");
        assert!(may_read(&hr, &team).await, "what is general flows into HR");
        assert!(!may_read(&secret, &hr).await, "what is HR's never into a general channel");

        let refused = session.create_channel(&alice_ctx, with("typo", classified(Confidentiality::Internal, &["hrr"]), vec![])).await.expect_err("not configured");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ClassificationRefused(_))), "{refused}");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_reader_knows_the_privacy_of_its_session_and_looks_up_others() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;

        let (_, manager) = owned_runtime(Default::default());
        let runtime = Arc::clone(runtime_of(&manager));
        let (alice, carol) = (user("alice"), user("carol"));
        let (alice_id, carol_id) = (ActorId::User(alice.id), ActorId::User(carol.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());

        let mut hers = make_session("hers", alice_id);
        hers.participants = record_users(&manager, &[&alice]).await;
        hers.privacy = PrivacyConfig::SameOwner;
        let hers = create(&manager, &alice_ctx, hers).await.unwrap();
        let theirs = make_session("theirs", carol_id);
        persist(&runtime, &SecurityContext::new(carol_id, Default::default(), Default::default()), &theirs).await;

        let channel = Arc::clone(&hers.channels()[0]);
        let views = views_in(&hers, &channel, alice_id);
        let session = views.session(&alice_ctx, channel.id(), channel.classification()).await.expect("attached to its session");
        let stranger = SecurityContext::new(ActorId::Agent(caelumora_model::agent::AgentId::new()), Default::default(), Default::default());
        assert!(views.session(&stranger, channel.id(), channel.classification()).await.is_none(), "reached by who may read it alone (issue 304)");

        let own = session.privacy();
        assert_eq!((own.policy, own.owner), (PrivacyConfig::SameOwner, alice_id));
        assert_eq!(own.people, [alice.id].into_iter().collect(), "its users and its owner");
        let other = session.views().privacy_of(theirs.id).await.unwrap().expect("stored, running or not");
        assert_eq!((other.policy, other.owner), (PrivacyConfig::Private, carol_id), "even a session the reader may not read");
        assert!(session.views().privacy_of(SessionId::new()).await.unwrap().is_none(), "one that is gone");
        manager.stop_session(&SecurityContext::system(), hers.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_released_message_is_read_where_its_channel_is_not() {
        use crate::view::ViewError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let alice = user("alice");
        let alice_id = ActorId::User(alice.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers", alice_id);
        session.participants = record_users(&manager, &[&alice]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification| ChannelSpec { classification: Some(classification), ..spec(name, vec![]) };
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]))).await.unwrap();
        let finance = session.create_channel(&alice_ctx, with("finance", classified(Confidentiality::Internal, &["finance"]))).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]))).await.unwrap();

        let writer = hr.user_actor(alice.id).expect("the creator takes part");
        let mut events = writer.watch().events;
        writer.send_message("for HR".to_string(), None).await.unwrap();
        writer.send_message("for finance too".to_string(), Some(classified(Confidentiality::Internal, &["hr", "finance"]))).await.unwrap();
        writer.send_message("too secret".to_string(), Some(classified(Confidentiality::Confidential, &["hr"]))).await.expect("sent: the channel decides");
        assert_eq!(rejection(&mut events).await.1, Rejection::ClassificationRefused, "above the channel, and its writer is told (issue 304)");
        let history = history_of(&hr, 2).await;
        assert_eq!(history[0].classification, classified(Confidentiality::Internal, &["hr"]), "the channel's, unless classified otherwise");

        // Read as an agent in another channel reads: through its turn's views.
        let reading_from = |channel: &Arc<ChannelLoop>| {
            let views = views_in(&session, channel, alice_id);
            let (context, channel_id, classification) = (alice_ctx.clone(), channel.id(), channel.classification());
            async move { views.session(&context, channel_id, classification).await.expect("attached to its session") }
        };
        let read = reading_from(&finance).await.channel_named("hr").await.expect("reachable for what was released").history().await.expect("what was released to finance");
        let texts: Vec<_> = read.messages().iter().map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(texts, ["for finance too"], "only what was released");
        let nothing = reading_from(&team).await.channel_named("hr").await.expect("reachable, but").history().await;
        assert!(matches!(nothing, Err(ViewError::NoChannelNamed(_))), "nothing was released to a general channel");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// A model whose answer never comes.
    #[derive(Debug)]
    struct Silent;

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Silent {
        async fn chat_completion(&self, _: &[caelumora_model::message::Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            Box::pin(futures::stream::pending())
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    #[tokio::test]
    async fn a_member_s_answer_under_way_is_in_the_snapshot_of_its_channel() {
        use caelumora_model::actor::{ActorEvent, ActorId};
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::scope::Scope;
        use caelumora_model::snapshot::Detail;
        

        let store = Arc::new(MemoryStore::new());
        let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
        registry.register(crate::component::ComponentKind::new("silent").provides::<dyn crate::llm::LLMClient>(), |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::llm::LLMClient>(Arc::new(Silent));
            Ok(facets)
        });
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), registry).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();

        let alice = user("alice");
        let participants = record_users(&manager, &[&alice]).await;
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("thinker", Default::default()).unwrap(), kind: PersonalityKind::Internal,
            roles: None, description: None, introduction: None,
            components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "silent")] },
            security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let thinker = Agent::new(ScopedName::new("thinker", Scope::GlobalScope).unwrap(), &personality, ActorId::User(alice.id));
        store.entity_stores().agents.create(&system, &thinker).await.unwrap();
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking", ActorId::None);
        session.participants = participants;
        session.participants.push(caelumora_model::session::Participant::Agent(thinker.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session.create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), agent])).await.unwrap();
        let mut events = channel.user_actor(alice.id).unwrap().watch().events;
        let shown = |events: &mut tokio::sync::broadcast::Receiver<caelumora_model::actor::Envelope>, wanted: &'static str| {
            let mut events = events.resubscribe();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let envelope = events.recv().await.expect("the channel is running");
                        if envelope.context.sender_id == agent && envelope.event.kind() == wanted { break envelope.event }
                    }
                }).await.unwrap_or_else(|_| panic!("the agent sends {wanted}"))
            }
        };
        let member = || async {
            let view = manager.views(Viewer::new(alice_ctx.clone())).session(session.id()).await.unwrap().channel(channel.id()).await.unwrap();
            view.snapshot(Detail::MEMBERS).await.members.into_iter().find(|m| m.id == agent).expect("a member")
        };
        assert_eq!(member().await.turn_under_way, None, "nothing under way yet");

        let started = shown(&mut events, "ResponseStarted");
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let ActorEvent::ResponseStarted { turn_id } = started.await else { unreachable!() };
        assert_eq!(member().await.turn_under_way, Some(turn_id), "the turn its start announced");

        let ended = shown(&mut events, "TurnFinished");
        channel.cancel_actor(&alice_ctx, agent).await.unwrap();
        ended.await;
        assert_eq!(member().await.turn_under_way, None, "over once it ended");
        manager.stop_session(&system, session.id()).await.unwrap();
    }

}
