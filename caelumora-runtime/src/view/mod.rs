//! Views: the sessions, their channels and their participants, as one viewer
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
//! session, reached through its situation (`Situation::session_view`); a
//! frontend's root is [`SessionManager::views`].
//!
//! A node answers with a snapshot (`caelumora_model::snapshot`, issue 80)
//! for a client that lists and offers: the node's data with its questions
//! answered at one moment.

mod agent;
mod channel;
mod context;
mod group;
mod participant;
mod project;
mod session;
mod workspace;

pub use agent::AgentView;
pub use channel::{ChannelView, ParticipantView};
pub use context::{AgentContextError, AgentContextView};
pub use group::GroupView;
pub use project::ProjectView;
pub use session::{PrivateNotesElsewhere, SessionView};
pub use workspace::WorkspaceView;

use crate::actor::ActorManager;
use crate::attachment::{AttachmentError, AttachmentManager};
use crate::component::ComponentRegistry;
use crate::controller::ControllerManager;
use crate::runtime::{RuntimeManager, Services};
use crate::session::{Found, SessionLoop, SessionManager};
use crate::workspace::WorkspaceManager;
use anyhow::Error;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality};
use caelumora_model::confidentiality::{
    ConfidentialityPolicy, ConfidentialityQuestions, Place, Reading,
};
use caelumora_model::group::{Group, GroupId};
use caelumora_model::message::MessageId;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::project::ProjectId;
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{
    AccessPolicy, AccessQuestions, AclEntry, Refused, SecurityAttributes, SecurityContext, Subject,
};
use caelumora_model::session::{ChannelId, SessionField, SessionId};
use caelumora_model::snapshot::{
    AccessSnapshot, DeletedSnapshot, PersonalitySnapshot, ScopeSnapshot,
};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::transcript::TranscriptStore;
use caelumora_model::store::{
    Deleted, EntityStore, EntityStores, StateStore, StoreResult, StoreResultExt, Stored,
};
use caelumora_model::workspace::WorkspaceId;
use std::error;
use std::fmt::Debug;
use std::iter;
use std::sync::Arc;
use std::{fmt, result};

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
    #[error("There is no project {0}")]
    ProjectNotFound(ProjectId),
    #[error("There is no group {0}")]
    GroupNotFound(GroupId),
    #[error("There is no agent {0}")]
    AgentNotFound(AgentId),
    /// Asked for by name, as an agent's tools do: no such channel, or
    /// nothing in it that can be read from where the viewer reads.
    #[error("there is no channel named \"{0}\" that can be read from here")]
    NoChannelNamed(String),
    #[error("Channel {channel} has no participant {actor}")]
    ParticipantNotFound { channel: ChannelId, actor: ActorId },
    /// Not in the history, or not in the part of it the viewer sees.
    #[error("Channel {channel} has no message {message}")]
    MessageNotFound { channel: ChannelId, message: MessageId },
    /// Reached, but not the viewer's to read: what an agent saw, which
    /// tools it has.
    #[error("You are not permitted to read this of {actor}")]
    Forbidden {
        actor: ActorId,
        #[source]
        refused: Refused,
    },
    /// Not the viewer's to do: searching whom to take into a session they
    /// may not change (issue 221).
    #[error("You are not permitted to do this")]
    NotPermitted(#[source] Refused),
    /// Only agents have a context.
    #[error("There is nothing {actor} saw: it is no agent, or no longer exists")]
    NoContext { actor: ActorId },
    /// An attachment of a message the viewer sees could not be read: it carries none such, or its
    /// storage failed (issue 282).
    #[error(transparent)]
    Attachment(#[from] AttachmentError),
    /// Reaching it failed; the cause is in the log.
    #[error("Reading failed")]
    Failed(#[source] Error),
}

impl ViewError {
    /// Reaching something failed: `error`, with what was being done, for
    /// the log - the viewer learns only that it failed.
    pub(crate) fn failed<E>(error: E, doing: impl fmt::Display + Send + Sync + 'static) -> Self
    where
        E: error::Error + Send + Sync + 'static,
    {
        ViewError::Failed(Error::new(error).context(doing))
    }
}

pub type ViewResult<T> = result::Result<T, ViewError>;

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
/// what it opens: the viewer's `context`, the `access_policy` and
/// `confidentiality` policies, the runtime's `service`s - among them the
/// plugin's manager - and `channel_reading`.
#[derive(Clone)]
pub struct Views {
    services: Services,
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    stores: EntityStores,
    states: Arc<dyn StateStore>,
    /// Where every history is read, in pages (issue 379).
    histories: Arc<dyn HistoryStore>,
    /// Where what an agent was given on an older branch of its transcript
    /// is read (issue 380).
    transcripts: Arc<dyn TranscriptStore>,
    /// The texts what an agent saw is worded with again (issue 57).
    prompts: Arc<PromptCatalogue>,
    components: Arc<ComponentRegistry>,
    workspaces: Arc<WorkspaceManager>,
    /// Where a session's workspace is asked whether it may be used in a
    /// channel (issue 344).
    controllers: Arc<ControllerManager>,
    /// Where a user follows a channel through, running or not (issue 386).
    actors: Arc<ActorManager>,
    /// Where the attachments of messages are read (issue 282).
    attachments: Arc<AttachmentManager>,
    running: Arc<dyn RunningSessions>,
    viewer: Viewer,
}

impl Views {
    pub(crate) fn new(
        runtime: &RuntimeManager,
        running: Arc<dyn RunningSessions>,
        viewer: Viewer,
    ) -> Self {
        Self {
            services: runtime.services().clone(),
            access_policy: Arc::clone(runtime.access_policy()),
            confidentiality: Arc::clone(runtime.confidentiality()),
            stores: runtime.entity_stores().clone(),
            states: Arc::clone(runtime.state_store()),
            histories: Arc::clone(runtime.history_store()),
            transcripts: Arc::clone(runtime.transcript_store()),
            prompts: Arc::clone(runtime.prompts()),
            components: Arc::clone(runtime.component_registry()),
            workspaces: Arc::clone(runtime.workspace_manager()),
            controllers: Arc::clone(runtime.controller_manager()),
            actors: Arc::clone(runtime.actor_manager()),
            attachments: Arc::clone(runtime.attachment_manager()),
            running,
            viewer,
        }
    }

    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    /// The views of an agent in `session`, which it reaches alone (issue
    /// 229): what `SessionLoop::views` gives.
    pub(crate) fn of_own_session(
        runtime: &RuntimeManager,
        session: Arc<SessionLoop>,
        viewer: Viewer,
    ) -> Self {
        Self::new(runtime, Arc::new(OwnSession(session)), viewer)
    }

    /// The viewer's context: in whose name every edge asks.
    pub fn context(&self) -> &SecurityContext {
        self.viewer.context()
    }

    /// The policy every edge asks, in the viewer's `context`.
    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        &self.access_policy
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

    pub(crate) fn histories(&self) -> &Arc<dyn HistoryStore> {
        &self.histories
    }

    pub(crate) fn transcripts(&self) -> &Arc<dyn TranscriptStore> {
        &self.transcripts
    }

    /// The texts given to models (issue 57).
    pub(crate) fn prompts(&self) -> &Arc<PromptCatalogue> {
        &self.prompts
    }

    pub(crate) fn components(&self) -> &ComponentRegistry {
        &self.components
    }

    pub(crate) fn workspace_manager(&self) -> &Arc<WorkspaceManager> {
        &self.workspaces
    }

    /// The controllers of this instance: a session's workspace among them.
    /// Where what of an actor outlives its run is kept (issue 386).
    pub(crate) fn actor_manager(&self) -> &Arc<ActorManager> {
        &self.actors
    }

    pub(crate) fn controller_manager(&self) -> &Arc<ControllerManager> {
        &self.controllers
    }

    pub(crate) fn attachment_manager(&self) -> &Arc<AttachmentManager> {
        &self.attachments
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        self.sessions_of(&Query::all()).await
    }

    /// The sessions `query` finds that the viewer may read, in the order the
    /// store lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub(super) async fn sessions_of(
        &self,
        query: &Query<SessionField>,
    ) -> ViewResult<Vec<SessionView>> {
        let sessions = self
            .stores
            .sessions
            .list(self.viewer.context(), query)
            .await
            .map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the sessions"))?;
        // The store lists what the viewer may read; a running session is
        // reached through its loop.
        Ok(sessions
            .into_iter()
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
                if !self.access_policy.may_read_session(self.viewer.context(), &attributes).await {
                    return Err(not_found());
                }
                Found::Running(session_loop)
            }
            None => Found::Stored(
                self.stores.sessions.get(self.viewer.context(), id).await.map_err(|e| {
                    if e.is_absent_or_refused() {
                        not_found()
                    } else {
                        ViewError::Failed(e.into())
                    }
                })?,
            ),
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
    pub async fn channel_of(
        &self,
        session: SessionId,
        channel: ChannelId,
    ) -> ViewResult<Reached<ChannelView>> {
        let session_view = match self.session(session).await {
            Ok(session_view) => session_view,
            Err(ViewError::SessionNotFound(_)) => return self.gone_session(session).await,
            Err(e) => return Err(e),
        };
        match session_view.channel(channel).await {
            Ok(channel) => Ok(Reached::There(channel)),
            Err(ViewError::ChannelNotFound { .. }) => {
                self.gone_channel(&session_view, channel).await
            }
            Err(e) => Err(e),
        }
    }

    /// Whether session `id` was deleted and the viewer could read it were
    /// it there.
    async fn gone_session<T>(&self, id: SessionId) -> ViewResult<Reached<T>> {
        let deleted = self
            .stores
            .sessions
            .get_deleted(&SecurityContext::system(), id)
            .await
            .optional()
            .map_err(|e| ViewError::failed(e, format!("looking up session {id}")))?;
        Ok(match deleted {
            Some(Deleted { entity: session, removal })
                if self
                    .access_policy
                    .may_read_session(self.viewer.context(), &session.security_attributes)
                    .await =>
            {
                Reached::Gone(removal)
            }
            _ => Reached::Never,
        })
    }

    /// Whether channel `id` of `session` was deleted and the viewer could
    /// read it were it there: reached as a live channel is
    /// (`SessionView::readable`, issue 304) - one the viewer is not cleared
    /// for is never there, as a live one would be not found.
    async fn gone_channel<T>(
        &self,
        session: &SessionView,
        id: ChannelId,
    ) -> ViewResult<Reached<T>> {
        let deleted = self
            .stores
            .channels
            .get_deleted(&SecurityContext::system(), id)
            .await
            .optional()
            .map_err(|e| ViewError::failed(e, format!("looking up channel {id}")))?;
        Ok(match deleted {
            Some(Deleted { entity: channel, removal })
                if channel.session_id == session.id()
                    && session.reading(&channel).await.is_some() =>
            {
                Reached::Gone(removal)
            }
            _ => Reached::Never,
        })
    }

    /// The agents the viewer may read (issues 164, 227), in the order the
    /// store lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn agents(&self) -> ViewResult<Vec<AgentView>> {
        let agents = self
            .stores
            .agents
            .list(self.viewer.context(), &Query::all())
            .await
            .map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the agents"))?;
        Ok(agents.into_iter().map(|agent| AgentView::of(self.clone(), agent)).collect())
    }

    /// Agent `id`, if the viewer may read it (`GetAgent`).
    ///
    /// # Errors
    /// `AgentNotFound` if there is none they may read.
    pub async fn agent(&self, id: AgentId) -> ViewResult<AgentView> {
        match self.stores.agents.get(self.viewer.context(), id).await.optional() {
            Ok(Some(agent)) => Ok(AgentView::of(self.clone(), agent)),
            Ok(None) => Err(ViewError::AgentNotFound(id)),
            Err(e) => Err(ViewError::failed(e, "reading an agent")),
        }
    }

    /// The personalities the viewer may read (`GetPersonality`, issue 227) -
    /// those they may make an agent of -, by name, so that every client
    /// shows them in the same order.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn personalities(&self) -> ViewResult<Vec<PersonalitySnapshot>> {
        let personalities: Vec<Personality> = self
            .stores
            .personalities
            .list(self.viewer.context(), &Query::all())
            .await
            .map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the personalities"))?;
        let mut listed = Vec::with_capacity(personalities.len());
        for personality in personalities {
            listed.push(PersonalitySnapshot {
                id: personality.id,
                name: personality.name.to_string(),
                scope: self.scope_snapshot(&personality.name.scope).await,
                kind: personality.kind,
                description: personality.description,
            });
        }
        listed.sort_by(|l, r| l.name.cmp(&r.name));
        Ok(listed)
    }

    /// The agents the viewer deleted and may read as such (issue 164). An
    /// agent is purged as the policy allows.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_agents(&self) -> ViewResult<Vec<DeletedSnapshot<AgentId>>> {
        self.deleted_of(self.stores.agents.as_ref(), &Query::all(), |agent| agent.name.to_string())
            .await
            .map_err(|e| ViewError::failed(e, "listing the deleted agents"))
    }

    /// The workspaces the store lists for the viewer (issues 23, 322) -
    /// their own and those shared with them, wherever they lie, that they
    /// are cleared for -, in the order it lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn workspaces(&self) -> ViewResult<Vec<WorkspaceView>> {
        let workspaces = self
            .stores
            .workspaces
            .list(self.viewer.context(), &Query::all())
            .await
            .map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the workspaces"))?;
        Ok(workspaces
            .into_iter()
            .map(|workspace| WorkspaceView::of(self.clone(), workspace))
            .collect())
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

    /// The shared projects the store lists for the viewer (issue 467) - their own, those they
    /// work on and those shared with them -, in the order it lists them. A session's own is
    /// reached through its session, not listed here.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn projects(&self) -> ViewResult<Vec<ProjectView>> {
        let projects = self
            .stores
            .projects
            .list(self.viewer.context(), &Query::filtered(Filter::shared_projects()))
            .await
            .map(|page| page.items)
            .map_err(|e| ViewError::failed(e, "listing the projects"))?;
        Ok(projects.into_iter().map(|project| ProjectView::of(self.clone(), project)).collect())
    }

    /// Project `id`, if the store lets the viewer read it (`GetProject`): a shared one, or the
    /// own of a session they read.
    ///
    /// # Errors
    /// `ProjectNotFound` if there is none they may read.
    pub async fn project(&self, id: ProjectId) -> ViewResult<ProjectView> {
        match self.stores.projects.get(self.viewer.context(), id).await.optional() {
            Ok(Some(project)) => Ok(ProjectView::of(self.clone(), project)),
            Ok(None) => Err(ViewError::ProjectNotFound(id)),
            Err(e) => Err(ViewError::failed(e, "reading a project")),
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
        let teams = self
            .teams()
            .await?
            .into_iter()
            .filter(|team| team.has_member(principal))
            .map(|team| Subject::Group(team.id));
        // Every user is named by an entry for everyone (issue 221).
        let everyone = matches!(principal, ActorId::User(_)).then_some(Subject::Everyone);
        Ok(iter::once(Subject::Actor(principal))
            .chain(everyone)
            .chain(roles.map(Subject::Role))
            .chain(teams)
            .collect())
    }

    /// The entries `acl` with whom each names called by name (issue 23),
    /// read in the runtime's name: shown to the owner of what they are of,
    /// who may name a session or user they cannot read themselves. A group is called as
    /// `group_name` calls it, and says where it lies.
    pub async fn access_snapshots(&self, acl: &[AclEntry]) -> Vec<AccessSnapshot> {
        let system = SecurityContext::system();
        let mut access = Vec::with_capacity(acl.len());
        for entry in acl {
            let (name, scope) = match entry.subject {
                Subject::Actor(ActorId::User(user)) => (
                    self.stores
                        .users
                        .get(&system, user)
                        .await
                        .optional()
                        .ok()
                        .flatten()
                        .map(|u| u.username.to_string()),
                    None,
                ),
                Subject::Actor(_) | Subject::Everyone => (None, None),
                Subject::Role(role) => (
                    self.confidentiality
                        .roles()
                        .and_then(|roles| roles.name_of(role))
                        .map(str::to_string),
                    None,
                ),
                Subject::Group(group) => {
                    match self.stores.groups.get(&system, group).await.optional().ok().flatten() {
                        Some(group) => (self.group_name(&group).await, Some(group.name.scope)),
                        None => (None, None),
                    }
                }
            };
            access.push(AccessSnapshot { subject: entry.subject, grant: entry.grant, name, scope });
        }
        access
    }

    /// How `group` is called: a team by its name, the participants of a session or a channel by
    /// the session's or channel's, the members of a project by the project's - of a session's own
    /// by the session's, since every own is called alike (issue 467) -, read in the runtime's name.
    async fn group_name(&self, group: &Group) -> Option<String> {
        let system = SecurityContext::system();
        let session_name = async |id| {
            self.stores
                .sessions
                .get(&system, id)
                .await
                .optional()
                .ok()
                .flatten()
                .map(|s| s.name.to_string())
        };
        match group.name.scope {
            Scope::SessionScope { session_id } => session_name(session_id).await,
            Scope::ChannelScope { channel_id, .. } => self
                .stores
                .channels
                .get(&system, channel_id)
                .await
                .optional()
                .ok()
                .flatten()
                .map(|c| c.name.to_string()),
            Scope::ProjectScope { project_id } => {
                let project = self
                    .stores
                    .projects
                    .get(&system, project_id)
                    .await
                    .optional()
                    .ok()
                    .flatten()?;
                match project.name.scope {
                    Scope::SessionScope { session_id } => session_name(session_id).await,
                    _ => Some(project.name.to_string()),
                }
            }
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
        let query = Query::filtered(Filter::teams_of(self.viewer.context().principal_id));
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

    /// The teams the viewer deleted and may read as such. The participants
    /// of a session or channel are deleted and restored with it.
    ///
    /// # Errors
    /// `Failed` if the store fails.
    pub async fn deleted_groups(&self) -> ViewResult<Vec<DeletedSnapshot<GroupId>>> {
        self.deleted_of(self.stores.groups.as_ref(), &Query::filtered(Filter::teams()), |group| {
            group.name.to_string()
        })
        .await
        .map_err(|e| ViewError::failed(e, "listing the deleted groups"))
    }

    /// The workspaces the viewer deleted and may read as such (issue 253).
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_workspaces(&self) -> ViewResult<Vec<DeletedSnapshot<WorkspaceId>>> {
        self.deleted_of(self.stores.workspaces.as_ref(), &Query::all(), |workspace| {
            workspace.name.to_string()
        })
        .await
        .map_err(|e| ViewError::failed(e, "listing the deleted workspaces"))
    }

    /// The shared projects deleted that the viewer may read as such (issue 467); a session's own
    /// comes back with its session.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_projects(&self) -> ViewResult<Vec<DeletedSnapshot<ProjectId>>> {
        self.deleted_of(
            self.stores.projects.as_ref(),
            &Query::filtered(Filter::shared_projects()),
            |project| project.name.to_string(),
        )
        .await
        .map_err(|e| ViewError::failed(e, "listing the deleted projects"))
    }

    /// The sessions the viewer deleted and may read as such (issue 164),
    /// in the order the store lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_sessions(&self) -> ViewResult<Vec<DeletedSnapshot<SessionId>>> {
        self.deleted_of(self.stores.sessions.as_ref(), &Query::all(), |session| {
            session.name.to_string()
        })
        .await
        .map_err(|e| ViewError::failed(e, "listing the deleted sessions"))
    }

    /// The deleted of kind `T` that `query` lists for the viewer in `store` (issues 164, 472), each
    /// as an entry called by `name`, with whether they may restore and purge it, in the order the
    /// store lists them: the one place an entry of what was deleted is made, for every kind - the
    /// plugins' too.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub async fn deleted_of<T: Stored>(
        &self,
        store: &dyn EntityStore<T>,
        query: &Query<T::Field>,
        name: impl Fn(&T) -> String + Send,
    ) -> StoreResult<Vec<DeletedSnapshot<T::Id>>> {
        let context = self.viewer.context();
        let mut deleted = Vec::new();
        for Deleted { entity, removal } in store.list_deleted(context, query).await?.items {
            let attributes = entity.security_attributes();
            deleted.push(DeletedSnapshot {
                id: entity.id(),
                name: name(&entity),
                // One whose purge is begun is restored by nobody (issue 492).
                can_restore: !removal.purging
                    && self.access_policy.may_restore(context, T::KIND, attributes).await,
                deleted: removal,
                can_purge: self.access_policy.may_purge(context, T::KIND, attributes).await,
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

    /// Where `scope` is, as the viewer is shown it (issue 475): with the name of its user if it is
    /// a user's - whose it is, read as `name_of` reads it.
    pub async fn scope_snapshot(&self, scope: &Scope) -> ScopeSnapshot {
        let user_name = match scope {
            Scope::UserScope { user_id } => self.name_of(ActorId::User(*user_id)).await,
            _ => None,
        };
        ScopeSnapshot { scope: scope.clone(), user_name }
    }

    /// The name of `actor`; `None` for a user or agent that no longer exists. Read in the runtime's
    /// name: taking part in a session makes a name known to everybody in it - it stands at every
    /// message - even where the record itself is not theirs to read (issue 120); so does owning an
    /// agent there, which acts with its owner's authority (issue 244), and reading something of a
    /// user's scope, which is theirs (issue 475).
    async fn name_of(&self, actor: ActorId) -> Option<String> {
        match actor {
            ActorId::User(id) => self
                .stores
                .users
                .get(&SecurityContext::system(), id)
                .await
                .ok()
                .map(|user| user.username.to_string()),
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
        self.stores
            .agents
            .get_deleted(&SecurityContext::system(), id)
            .await
            .ok()
            .map(|agent| agent.entity.name.to_string())
    }

    /// Who deleted agent `id` and when, if it was deleted and not purged
    /// (issue 314): when it left the channels it rests in. Read in the
    /// runtime's name, like its name.
    pub(super) async fn agent_removal(&self, id: AgentId) -> Option<Removal> {
        self.stores
            .agents
            .get_deleted(&SecurityContext::system(), id)
            .await
            .ok()
            .map(|agent| agent.removal)
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
    pub async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>> {
        let session = self
            .stores
            .sessions
            .get(&SecurityContext::system(), id)
            .await
            .optional()
            .map_err(|e| ViewError::failed(e, format!("looking up session {id}")))?;
        Ok(session.as_ref().map(SessionPrivacy::of))
    }
}

/// How someone reads a channel (issue 133): an agent from its own place -
/// its channel, the classification it reads at, its session's privacy -,
/// a user by what the roles of their context clear them for.
#[derive(Debug, Clone, Copy)]
pub enum ChannelReader<'a> {
    From(&'a Place),
    Cleared,
}

impl<'a> ChannelReader<'a> {
    /// The reader who reads from `from`, if it reads from a place as an agent does, and by its
    /// clearance otherwise: the one rule by which a reader is chosen.
    pub fn of(from: Option<&'a Place>) -> Self {
        from.map_or(Self::Cleared, Self::From)
    }
}

/// How `context` may read the channel with `channel` attributes, which lies
/// `at` its place (issues 133, 264, 296, 304): not at all if it may not read
/// it (`GetChannel`, the `AccessPolicy`'s). Beyond that, as the
/// `ConfidentialityPolicy` decides: an agent reading from its place reads it
/// as a whole if what is there may be copied into that place, and otherwise
/// what of it may (`Reading`, issue 421), entry by entry; a user reads it as
/// a whole if they are cleared for its classification, and not at all
/// otherwise. What the views reach a channel by, and what an operation in a
/// channel asks before it acts there.
pub async fn channel_reading(
    access_policy: &dyn AccessPolicy,
    confidentiality: &dyn ConfidentialityPolicy,
    context: &SecurityContext,
    channel: &SecurityAttributes,
    at: &Place,
    reader: ChannelReader<'_>,
) -> Option<Reading> {
    if !access_policy.may_read_channel(context, channel).await {
        return None;
    }
    match reader {
        ChannelReader::From(from) => Some(
            match confidentiality.copyable_information(context, at.channel, &at.session, from) {
                reading if reading.admits(&at.classification, confidentiality.compartments()) => {
                    Reading::Whole
                }
                reading => reading,
            },
        ),
        ChannelReader::Cleared => {
            confidentiality.cleared_for(context, &at.classification).then_some(Reading::Whole)
        }
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

impl Debug for Views {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Views").field("viewer", &self.viewer).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Script, ScriptedAgents, TestRuntime};

    use crate::actor::user::ChannelWatch;
    use crate::channel::ChannelLoop;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError};
    use caelumora_model::actor::Rejection;
    use caelumora_model::agent::testing::make_personality;
    use caelumora_model::agent::{Agent, AgentChannelState, Personality, PersonalityKind};
    use caelumora_model::approval::ToolMatcher;
    use caelumora_model::channel::{Channel, ChannelState, Participant};
    use caelumora_model::confidentiality::{Classification, ConfidentialityRules};
    use caelumora_model::context::{ContextItem, ContextRef};
    use caelumora_model::history::{Audience, HistoryEntry, HistoryPage, Recorded};
    use caelumora_model::message::Message;
    use caelumora_model::security::{Grant, SecurityAttributes, Subject};
    use caelumora_model::session::{Participant as SessionParticipant, Session};
    use caelumora_model::snapshot::Detail;
    use caelumora_model::store::Persistence;
    use caelumora_model::turn::GivenUpReason;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::value::text::Introduction;
    use caelumora_model::value::text::{Briefing, Summary};
    use caelumora_model::work::TurnId;
    use chrono::Utc;
    use std::slice;
    use std::time::Duration;
    use tokio::time;

    /// All of the history `view` shows, as one page.
    async fn whole(view: &ChannelView) -> ViewResult<HistoryPage> {
        view.history_page(None, 10_000).await
    }

    /// The messages of `page`.
    fn messages(page: &HistoryPage) -> Vec<Message> {
        page.entries().filter_map(HistoryEntry::said).cloned().collect()
    }

    fn as_user(id: UserId) -> Viewer {
        Viewer::new(SecurityContext::new(ActorId::User(id), Default::default(), Default::default()))
    }

    /// `session` with `channels`, stored as the runtime stores a brand-new one (`persist`), the
    /// participants of each channel joined as its founders (`founded`): the session as stored.
    async fn stored(manager: &SessionManager, session: Session, channels: &[Channel]) -> Session {
        let session = persist(manager, &SecurityContext::system(), session, channels).await;
        for channel in channels {
            manager
                .runtime()
                .state_store()
                .put_channel_state(channel.id, &mut founded(channel))
                .await
                .unwrap();
        }
        session
    }

    /// Another channel like `template`, named `name`: an id of its own,
    /// read by its own participants.
    fn like(template: &Channel, name: &str) -> Channel {
        let id = ChannelId::new();
        Channel {
            id,
            name: Name::new(name).unwrap(),
            security_attributes: Channel::attributes_of(id, template.security_attributes.owner_id),
            ..template.clone()
        }
    }

    async fn an_agent_of(store: &Persistence, owner: ActorId) -> Agent {
        an_agent_of_kind(store, owner, Default::default()).await
    }

    async fn an_agent_of_kind(store: &Persistence, owner: ActorId, kind: PersonalityKind) -> Agent {
        let personality = Personality { kind, ..make_personality("plain") };
        store
            .entity_stores()
            .unwrap()
            .personalities
            .create(&SecurityContext::system(), &personality)
            .await
            .unwrap();
        let agent = Agent::new(
            ScopedName::new("Helper", Scope::GlobalScope).unwrap(),
            personality.id,
            owner,
        );
        store
            .entity_stores()
            .unwrap()
            .agents
            .create(&SecurityContext::system(), &agent)
            .await
            .unwrap();
        agent
    }

    #[tokio::test]
    async fn the_context_of_an_external_agent_is_what_the_remote_agent_was_given() {
        use caelumora_model::message::{Message, MessageId};

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let remote =
            an_agent_of_kind(&store, ActorId::User(alice.id), PersonalityKind::External).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(remote.id));
        let channel = main_channel(&session);
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        let said = |t: &str| Message {
            id: MessageId::new(),
            content: Some(t.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let question = said("question");
        let answer = Message {
            turn_id: Some(TurnId::new()),
            sender_id: ActorId::Agent(remote.id),
            ..said("answer")
        };
        let mut state = AgentChannelState::default();
        // One call: the question was given, the transcript started anew,
        // and the answer came of it.
        state.transcript.extend(vec![question.clone()]);
        let given = ContextRef { transcript: state.transcript.start_anew(), ..Default::default() };
        let frame = state.keep_frame(&given.frame);
        let later = said("later");
        state.transcript.extend(vec![later.clone()]);
        store
            .state_store()
            .put_agent_channel_state(remote.id, channel.id, &mut state)
            .await
            .unwrap();
        // Its channel's history, as it records what was said (issue 417).
        let mut channel_state = store.state_store().get_channel_state(channel.id).await.unwrap();
        channel_state.history.extend(vec![
            question.clone(),
            answer.clone().produced_from_context(given.transcript, frame),
            later,
        ]);
        store.state_store().put_channel_state(channel.id, &mut channel_state).await.unwrap();

        let for_alice = manager
            .views(as_user(alice.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap();
        let participant = for_alice.participant(ActorId::Agent(remote.id)).await.unwrap();
        let seen = participant.context().await.expect("alice made the agent");
        let texts = |messages: Vec<Message>| {
            messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>()
        };
        assert_eq!(
            texts(seen.transcript().messages()),
            ["later"],
            "what it is given next, not what it was given or answered"
        );
        assert_eq!(seen.private_notes(), (Vec::new(), None), "an external agent writes none");
        assert!(!seen
            .history_page(None, 100)
            .await
            .unwrap()
            .entries()
            .any(|item| matches!(item, HistoryEntry::TurnGivenUp { .. })));
        let context =
            seen.context_of(answer.id).await.expect("what the call was given (issue 205)");
        assert_eq!(context.items.len(), 1, "the messages of the one call, no prompt parts");
        assert!(
            matches!(&context.items[0], ContextItem::Conversation { message } if message.id == question.id)
        );
        assert!(context.tool_sets.is_empty(), "and no tools: those are the remote agent's");
        assert_eq!(
            seen.classification_of(answer.id).await.unwrap(),
            Some(Classification::CHANNEL_DEFAULT),
            "its answer is in its history"
        );
        assert_eq!(participant.private_notes().await.unwrap(), (Vec::new(), None));
        assert!(
            participant.notes().await.unwrap().expect("an agent").notes.is_empty(),
            "it writes none"
        );
        assert!(for_alice
            .notes()
            .await
            .unwrap()
            .iter()
            .all(|published| published.notes.is_empty()));
        assert!(
            !participant.may_retry().await,
            "it runs elsewhere and does not answer again (issue 242)"
        );
        assert!(!participant.snapshot(Detail::empty()).await.can_retry);
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn the_views_show_what_their_viewer_may_read_and_do_whether_the_session_runs_or_not() {
        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;

        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let main = main_channel(&session);
        let running =
            manager.create(as_user(alice.id).context(), session, vec![main]).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = running
            .create_channel(
                as_user(alice.id).context(),
                ChannelSpec {
                    name: "private".to_string(),
                    components: Default::default(),
                    classification: None,
                    language: None,
                    uses_workspace: None,
                    participants: vec![],
                    internal_actors: Vec::new(),
                },
            )
            .await
            .unwrap()
            .id();
        let id = running.id();

        let check = |state: &'static str| {
            let manager = &manager;
            async move {
                let names = |channels: &[ChannelView]| {
                    channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>()
                };
                let for_alice = manager.views(as_user(alice.id)).session(id).await.unwrap();
                assert_eq!(
                    names(&for_alice.channels().await),
                    ["main", "private"],
                    "{state}: the owner reads every channel"
                );
                assert_eq!(
                    for_alice
                        .snapshot(Detail::empty())
                        .await
                        .participants
                        .iter()
                        .filter_map(|p| p.name.as_deref())
                        .collect::<Vec<_>>(),
                    ["alice", "bob", "Helper"],
                    "{state}"
                );
                let for_bob = manager.views(as_user(bob.id)).session(id).await.unwrap();
                assert_eq!(
                    names(&for_bob.channels().await),
                    ["main"],
                    "{state}: bob is not in the private channel"
                );
                assert!(
                    for_alice.may_change().await && !for_bob.may_change().await,
                    "{state}: the owner changes the session, a participant does not"
                );

                let main = for_alice.channel(for_alice.channels().await[0].id()).await.unwrap();
                assert!(main.may_change().await, "{state}: the owner changes the channel");
                let agent = main
                    .participant(ActorId::Agent(helper.id))
                    .await
                    .expect("the agent is a participant");
                assert_eq!(agent.handle().as_deref(), Some("helper"), "{state}");
                assert!(agent.may_read_context().await, "{state}: alice made the agent");
                assert!(agent.may_cancel().await, "{state}: she may end its answer (issue 243)");
                assert_eq!(
                    agent.state().is_some(),
                    state == "running",
                    "{state}: where it stands, only while it runs"
                );
                let main = &for_bob.channels().await[0];
                assert!(!main.may_change().await, "{state}: a participant does not change it");
                for participant in main.participants().await {
                    assert!(
                        !participant.may_read_context().await,
                        "{state}: bob reads no agent's context"
                    );
                    assert_eq!(
                        participant.may_retry().await,
                        participant.id() == ActorId::Agent(helper.id),
                        "{state}: a participant asks an agent to answer again, nobody else"
                    );
                    assert_eq!(
                        participant.may_cancel().await,
                        participant.id() == ActorId::Agent(helper.id),
                        "{state}: and ends its answer, nobody else's"
                    );
                    assert!(
                        participant.id() == ActorId::Agent(helper.id)
                            || participant.state().is_none(),
                        "{state}: only an agent stands somewhere"
                    );
                }

                let hidden = for_bob.channel(hers).await.expect_err("not bob's to read");
                assert!(
                    matches!(hidden, ViewError::ChannelNotFound { .. }),
                    "{state}: absent, not refused: {hidden}"
                );
                assert_eq!(for_alice.channel(hers).await.unwrap().name(), "private", "{state}");
                let refused = manager
                    .views(as_user(carol.id))
                    .session(id)
                    .await
                    .expect_err("carol may not see it");
                assert!(matches!(refused, ViewError::SessionNotFound(_)), "{state}: {refused}");
                assert!(
                    manager.views(as_user(carol.id)).sessions().await.unwrap().is_empty(),
                    "{state}: nor is it listed for her"
                );
                let listed = manager.views(as_user(bob.id)).sessions().await.unwrap();
                assert_eq!(listed.len(), 1, "{state}");
                let all = Detail::all();
                assert_eq!(
                    listed[0].snapshot(all).await,
                    for_bob.snapshot(all).await,
                    "{state}: listed as it is reached"
                );
            }
        };
        check("running").await;
        assert!(manager.stop(&SecurityContext::system(), id).await.unwrap(), "it ran");
        check("stopped").await;
        assert!(manager.running(id).is_none(), "reaching a session does not start it");
    }

    #[tokio::test]
    async fn a_channel_is_described_with_handles_only_for_who_can_be_mentioned() {
        use caelumora_model::approval::{ApprovalStrategy, Approver};
        use caelumora_model::channel::InternalActor;

        let (_, manager) = owned_runtime(Default::default());
        let alice = user("Alice Example");
        let gone = UserId::new();
        let mut session = make_session("s", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        let mut channel = main_channel(&session);
        channel.participants.push(Participant::User(gone));
        channel.internal_actors.push(InternalActor::Approver(Approver::new(ApprovalStrategy::Any)));
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        let view = manager
            .views(as_user(alice.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap()
            .snapshot(Detail::PARTICIPANTS)
            .await;
        let [alice, gone] = view.participants.as_slice() else {
            panic!(
                "two participants, in order, and the approver none of them (issue 420): {view:?}"
            )
        };
        assert_eq!(
            (alice.name.as_deref(), alice.handle.as_deref()),
            (Some("Alice Example"), Some("alice-example"))
        );
        assert_eq!(
            (gone.name.as_deref(), gone.handle.as_deref()),
            (None, None),
            "a deleted user is listed, but cannot be mentioned"
        );
        assert!(
            view.participants.iter().all(|m| !m.can_read_context),
            "no participant is an agent"
        );
    }

    #[tokio::test]
    async fn what_a_place_is_for_is_shown_its_briefing_only_where_asked_for() {
        let (_, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        (session.summary, session.briefing) = (
            Some(Summary::new("Release 2.0").unwrap()),
            Some(Briefing::new("Dates are in UTC.").unwrap()),
        );
        let mut channel = main_channel(&session);
        (channel.summary, channel.briefing) =
            (Some(Summary::new("Planning").unwrap()), Some(Briefing::new("Be brief.").unwrap()));
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap();
        let listed = view.snapshot(Detail::CHANNELS).await;
        assert_eq!(
            (listed.summary.as_ref().map(Summary::as_str), listed.briefing.as_ref()),
            (Some("Release 2.0"), None),
            "the summary always, the briefing not where listed"
        );
        assert_eq!(
            (
                listed.channels[0].summary.as_ref().map(Summary::as_str),
                listed.channels[0].briefing.as_ref()
            ),
            (Some("Planning"), None)
        );
        let read = view.snapshot(Detail::CHANNELS | Detail::BRIEFING).await;
        assert_eq!(
            (
                read.briefing.as_ref().map(Briefing::as_str),
                read.channels[0].briefing.as_ref().map(Briefing::as_str)
            ),
            (Some("Dates are in UTC."), Some("Be brief."))
        );
    }

    #[tokio::test]
    async fn a_participants_profile_says_who_it_is_to_everybody_in_the_channel() {
        use caelumora_model::user::UserProfile;

        let (store, manager) = owned_runtime(Default::default());
        let system = SecurityContext::system();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        // Carol owns the agent but takes no part in the session.
        store.entity_stores().unwrap().users.create(&system, &carol).await.unwrap();
        let remote =
            an_agent_of_kind(&store, ActorId::User(carol.id), PersonalityKind::External).await;
        let mut personality = store
            .entity_stores()
            .unwrap()
            .personalities
            .get(&system, remote.personality_id)
            .await
            .unwrap();
        personality.introduction =
            Some(Introduction::new("Answers questions about the weather.").unwrap());
        store
            .entity_stores()
            .unwrap()
            .personalities
            .update(&system, &mut personality)
            .await
            .unwrap();
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(remote.id));
        let channel = main_channel(&session);
        store
            .entity_stores()
            .unwrap()
            .user_profiles
            .create(
                &system,
                &UserProfile::new(alice.id, Some(Introduction::new("I run the project.").unwrap())),
            )
            .await
            .unwrap();
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        let for_bob = manager
            .views(as_user(bob.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap();
        let plain = for_bob.snapshot(Detail::PARTICIPANTS).await;
        assert!(
            plain
                .participants
                .iter()
                .all(|m| m.introduction.is_none() && m.personality.is_none() && m.owner.is_none()),
            "only with PROFILE: {plain:?}"
        );

        let view = for_bob.snapshot(Detail::PARTICIPANTS | Detail::PROFILE).await;
        let [alice_seen, bob_seen, agent] = view.participants.as_slice() else {
            panic!("three participants, in order: {view:?}")
        };
        assert_eq!(
            alice_seen.introduction.as_deref(),
            Some("I run the project."),
            "as she introduces herself"
        );
        assert_eq!(
            (alice_seen.personality.as_deref(), alice_seen.owner),
            (None, None),
            "a user has neither personality nor owner"
        );
        assert_eq!(bob_seen.introduction, None, "bob did not introduce himself");
        assert_eq!(
            agent.introduction.as_deref(),
            Some("Answers questions about the weather."),
            "as its personality introduces it"
        );
        assert_eq!(
            (agent.personality.as_deref(), agent.personality_kind),
            (Some("plain"), Some(PersonalityKind::External)),
            "public, though bob may not read the personality"
        );
        assert_eq!(
            (agent.owner, agent.owner_name.as_deref()),
            (Some(ActorId::User(carol.id)), Some("carol")),
            "named although carol is not in the session"
        );

        let alone = for_bob
            .participant(ActorId::Agent(remote.id))
            .await
            .unwrap()
            .snapshot(Detail::PROFILE)
            .await;
        assert_eq!(&alone, agent, "a participant asked for alone is shown as in the list");
    }

    /// Issue 404: what is new to a user is what others said that they hear,
    /// after the last message they read and from where their view begins;
    /// the mark moves forward only, and only to a message they see.
    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn what_is_new_to_a_user_is_counted_from_what_they_read() {
        use caelumora_model::message::{Message, MessageId, MessageRole};
        use caelumora_model::snapshot::Unread;
        use caelumora_model::store::StoreError;
        use caelumora_model::user::User;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob, carol, dave) = (user("alice"), user("bob"), user("carol"), user("dave"));
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob, &carol, &dave]).await;
        let mut channel = main_channel(&session);
        channel
            .participants
            .retain(|participant| participant.actor_id() != ActorId::User(carol.id));
        let session =
            persist(&manager, &SecurityContext::system(), session, slice::from_ref(&channel)).await;

        let said = |by: &User, text: &str| Message {
            id: MessageId::new(),
            role: MessageRole::User,
            sender_id: ActorId::User(by.id),
            content: Some(text.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let hello = said(&dave, "hello");
        let hers = said(&alice, "hi");
        let step = Message { internal: true, ..said(&bob, "a step") };
        let notice = Message { role: MessageRole::System, ..said(&bob, "a notice") };
        let asking =
            Message { mentions: vec![ActorId::User(alice.id)], ..said(&bob, "@alice, a question") };
        // Alice and dave from the start, bob joined after dave's hello (issue 417).
        let joined = |who: &User| {
            Recorded::new(
                HistoryEntry::ParticipantJoined {
                    participant: ActorId::User(who.id),
                    name: who.username.to_string(),
                    by: ActorId::User(alice.id),
                    by_name: "alice".to_string(),
                    at: Utc::now(),
                },
                ActorId::User(alice.id),
                Audience::Everybody,
            )
        };
        let mut state = ChannelState::new(channel.id);
        state.history.enter(joined(&alice));
        state.history.enter(joined(&dave));
        state.history.push(hello.clone());
        state.history.enter(joined(&bob));
        state.history.extend(vec![hers.clone(), step.clone(), notice, asking.clone()]);
        store.state_store().put_channel_state(channel.id, &mut state).await.unwrap();

        let unread_of = |viewer: &User| {
            let (manager, viewer) = (&manager, viewer.id);
            async move {
                let channel = manager
                    .views(as_user(viewer))
                    .session(session.id)
                    .await
                    .unwrap()
                    .channel(channel.id)
                    .await
                    .unwrap();
                channel.snapshot(Detail::UNREAD).await.unread
            }
        };
        let unread =
            |messages, mentioned, read_up_to| Some(Unread { messages, mentioned, read_up_to });
        assert_eq!(
            unread_of(&alice).await,
            unread(2, true, None),
            "the others' words, not her own, bob's steps or the runtime's notices"
        );
        assert_eq!(
            unread_of(&bob).await,
            unread(1, false, None),
            "hers, from where his view begins"
        );
        let without = manager
            .views(as_user(alice.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap()
            .snapshot(Detail::empty())
            .await;
        assert_eq!(without.unread, None, "only where it is asked for");

        let controller = |user: &User| {
            let (manager, user) = (&manager, user.id);
            async move {
                Arc::clone(manager.views(as_user(user)).controller_manager())
                    .user_controller(user)
                    .await
                    .unwrap()
            }
        };
        let mark = |user: &User, message: MessageId| {
            let (controller, user) = (controller(user), user.id);
            async move {
                controller
                    .await
                    .mark_read(as_user(user).context(), session.id, channel.id, message)
                    .await
            }
        };
        mark(&alice, hello.id).await.expect("hers to mark");
        assert_eq!(unread_of(&alice).await, unread(1, true, Some(hello.id)), "after what she read");
        mark(&alice, asking.id).await.expect("hers to mark");
        assert_eq!(unread_of(&alice).await, unread(0, false, Some(asking.id)));
        mark(&alice, hello.id).await.expect("an older one is no error");
        assert_eq!(
            unread_of(&alice).await,
            unread(0, false, Some(asking.id)),
            "but does not move the mark back"
        );
        assert_eq!(
            store.state_store().get_user_channel_state(bob.id, channel.id).await.unwrap().read,
            None,
            "hers alone"
        );

        let not_found = |result: Result<(), StoreError>, entity: &str| matches!(result, Err(StoreError::NotFound { entity: found, .. }) if found == entity);
        assert!(
            not_found(mark(&bob, hello.id).await, "message"),
            "not before where his view begins"
        );
        assert!(
            not_found(mark(&bob, MessageId::new()).await, "message"),
            "nor what was never said"
        );
        assert!(
            not_found(mark(&carol, hello.id).await, "channel"),
            "nor in a channel she does not read"
        );
        let elsewhere = controller(&alice)
            .await
            .mark_read(as_user(alice.id).context(), SessionId::new(), channel.id, hers.id)
            .await;
        assert!(not_found(elsewhere, "channel"), "nor in another session");
        let forged = controller(&alice)
            .await
            .mark_read(as_user(bob.id).context(), session.id, channel.id, hers.id)
            .await;
        assert!(
            matches!(forged, Err(StoreError::Validation(_))),
            "nor by someone else: {forged:?}"
        );
        assert_eq!(
            unread_of(&alice).await,
            unread(0, false, Some(asking.id)),
            "none of it changed anything"
        );
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn what_a_channel_holds_is_read_through_its_nodes_and_a_participants_context_by_who_may()
    {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId, MessageRole};
        use caelumora_model::note::{NoteId, NoteOrigin};

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let session =
            persist(&manager, &SecurityContext::system(), session, slice::from_ref(&channel)).await;

        // What was said, of which bob - who joined later - sees the end only.
        let said = |t: &str| Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(t.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let (first, second) = (said("first"), said("second"));
        // And so does the agent, though alice, for whom it acts, sees it all
        // (issue 417).
        let alice_id = ActorId::User(alice.id);
        let joined = |who: ActorId| {
            Recorded::new(
                HistoryEntry::ParticipantJoined {
                    participant: who,
                    name: who.to_string(),
                    by: alice_id,
                    by_name: "alice".to_string(),
                    at: Utc::now(),
                },
                alice_id,
                Audience::Everybody,
            )
        };
        let mut state = ChannelState::new(channel.id);
        state.history.enter(joined(alice_id));
        state.history.push(first.clone());
        state.history.enter(joined(ActorId::User(bob.id)));
        state.history.enter(joined(ActorId::Agent(helper.id)));
        state.history.push(second.clone());
        store.state_store().put_channel_state(channel.id, &mut state).await.unwrap();
        // What the agent noted and saw.
        let mut agent_state = AgentChannelState::default();
        agent_state.notes.push(
            Change::Add {
                id: NoteId(1),
                text: "noted".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            NoteOrigin::default(),
            MessageId::default(),
        );
        agent_state.private_notes.push(
            Change::Add {
                id: NoteId(1),
                text: "mine".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            NoteOrigin::default(),
            MessageId::default(),
        );
        agent_state.transcript.push(first.clone());
        store
            .state_store()
            .put_agent_channel_state(helper.id, channel.id, &mut agent_state)
            .await
            .unwrap();

        let reach = |viewer: Viewer| {
            let manager = &manager;
            async move {
                manager
                    .views(viewer)
                    .session(session.id)
                    .await
                    .unwrap()
                    .channel(channel.id)
                    .await
                    .unwrap()
            }
        };
        let for_alice = reach(as_user(alice.id)).await;
        let texts = |messages: &[Message]| {
            messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
        };
        assert_eq!(texts(&messages(&whole(&for_alice).await.unwrap())), ["first", "second"]);
        assert_eq!(
            texts(&messages(&whole(&reach(as_user(bob.id)).await).await.unwrap())),
            ["second"],
            "bob joined after the first"
        );
        let as_helper = Viewer::new(
            SecurityContext::new(ActorId::User(alice.id), session.id, channel.id).next(
                ActorId::Agent(helper.id),
                ActorId::User(alice.id),
                ActorId::User(alice.id),
            ),
        );
        assert_eq!(
            texts(&messages(&whole(&reach(as_helper).await).await.unwrap())),
            ["second"],
            "the agent reads with its own limit, not its owner's"
        );
        // In pages (issue 218), with the same limits.
        let page_texts = |page: &HistoryPage| {
            page.entries()
                .filter_map(HistoryEntry::said)
                .filter_map(|m| m.content.clone())
                .collect::<Vec<_>>()
        };
        let last = for_alice.history_page(None, 1).await.unwrap();
        assert_eq!(page_texts(&last), ["second"]);
        // Before it the joins of bob and the agent, the first, and her own join.
        let before = for_alice.history_page(last.earlier, 4).await.unwrap();
        assert_eq!(
            (page_texts(&before), before.earlier),
            (vec!["first".to_string()], None),
            "the page before it, and nothing older"
        );
        let for_bob = reach(as_user(bob.id)).await;
        let his = for_bob.history_page(None, 10).await.unwrap();
        assert_eq!(
            (page_texts(&his), his.earlier),
            (vec!["second".to_string()], None),
            "bob's from where he joined"
        );
        assert_eq!(for_alice.message(first.id).await.unwrap().content.as_deref(), Some("first"));
        assert!(
            matches!(for_bob.message(first.id).await, Err(ViewError::MessageNotFound { .. })),
            "not before where his view begins"
        );
        assert_eq!(for_bob.message(second.id).await.unwrap().id, second.id);
        let notes = for_alice.notes().await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            (notes[0].agent, notes[0].name.as_str(), notes[0].notes[0].text.as_str()),
            (ActorId::Agent(helper.id), "Helper", "noted")
        );

        let agent = for_alice.participant(ActorId::Agent(helper.id)).await.unwrap();
        let seen = agent.context().await.expect("alice made the agent");
        assert_eq!(seen.private_notes().0[0].text, "mine");
        assert_eq!(seen.transcript().messages().len(), 1);
        assert!(
            matches!(seen.context_of(first.id).await, Err(AgentContextError::NotRecorded { .. })),
            "the agent did not produce it"
        );
        let refused = reach(as_user(bob.id))
            .await
            .participant(ActorId::Agent(helper.id))
            .await
            .unwrap()
            .context()
            .await
            .expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let none = for_alice
            .participant(ActorId::User(bob.id))
            .await
            .unwrap()
            .context()
            .await
            .expect_err("a user saw nothing");
        assert!(matches!(none, ViewError::NoContext { .. }), "{none}");
        assert!(matches!(
            for_alice.participant(ActorId::System).await,
            Err(ViewError::ParticipantNotFound { .. })
        ));
    }

    /// An agent reads another channel of its session only where its owner
    /// may read it or it takes part in it itself (issue 264): bob's agent,
    /// which left alice's channel "hers", does not reach it from "team" -
    /// though what it holds is classified as "team" is - but still knows
    /// what it wrote there itself, its private notes, through itself.
    #[tokio::test]
    async fn an_agent_does_not_reach_a_channel_its_owner_may_not_read_but_knows_what_it_wrote_there(
    ) {
        use caelumora_model::channel::FormerParticipant;
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::privacy::SessionPrivacy;
        use caelumora_model::removal::Removal;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(bob.id)).await;
        let mut session = make_session("shared", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        let team = like(&template, "team");
        let hers = Channel {
            id: ChannelId::new(),
            name: Name::new("hers").unwrap(),
            participants: vec![Participant::User(alice.id)],
            security_attributes: SecurityAttributes::owned_by(ActorId::User(alice.id)),
            former: vec![FormerParticipant {
                participant: Participant::Agent(helper.id),
                left: Removal::now(ActorId::User(alice.id)),
            }],
            ..template.clone()
        };
        session.channels = vec![team.id, hers.id];
        let session = stored(&manager, session, &[team.clone(), hers.clone()]).await;
        // Classified as team is: the flow rule alone would let team read it all.
        let mut state = store.state_store().get_channel_state(hers.id).await.unwrap();
        state.history.extend(vec![Message {
            id: MessageId::new(),
            content: Some("hers alone".to_string()),
            ..Message::new(hers.classification.clone())
        }]);
        store.state_store().put_channel_state(hers.id, &mut state).await.unwrap();
        let mut agent_state = AgentChannelState::default();
        let note = |text: &str| Change::Add {
            id: NoteId(1),
            text: text.to_string(),
            classification: hers.classification.clone(),
        };
        agent_state.notes.push(note("noted for all"), NoteOrigin::default(), MessageId::default());
        agent_state.private_notes.push(
            note("noted for itself"),
            NoteOrigin::default(),
            MessageId::default(),
        );
        store
            .state_store()
            .put_agent_channel_state(helper.id, hers.id, &mut agent_state)
            .await
            .unwrap();

        let context = SecurityContext::new(ActorId::User(bob.id), session.id, team.id).next(
            ActorId::Agent(helper.id),
            ActorId::User(bob.id),
            ActorId::User(bob.id),
        );
        let from = Place {
            channel: team.id,
            classification: team.classification.clone(),
            session: SessionPrivacy::of(&session),
        };
        let from_team =
            manager.views(Viewer::from_place(context, from)).session(session.id).await.unwrap();
        let names: Vec<String> =
            from_team.channels().await.iter().map(|c| c.name().to_string()).collect();
        assert_eq!(names, ["team"], "not hers as a whole");
        assert!(
            matches!(from_team.channel_named("hers").await, Err(ViewError::NoChannelNamed(_))),
            "not reached at all: it answers as absent"
        );
        assert!(
            matches!(from_team.channel(hers.id).await, Err(ViewError::ChannelNotFound { .. })),
            "also by its id"
        );
        assert!(
            from_team.all_channels().await.iter().all(|c| c.id() != hers.id),
            "nor entry by entry"
        );
        let own: Vec<_> = from_team
            .own_private_notes()
            .await
            .into_iter()
            .map(|found| (found.name, found.notes[0].text.clone()))
            .collect();
        assert_eq!(
            own,
            [("hers".to_string(), "noted for itself".to_string())],
            "what it wrote there itself, by the flow rule alone"
        );
    }

    #[tokio::test]
    async fn an_agents_private_notes_are_read_from_every_channel_of_the_session_by_who_may() {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::note::{NoteId, NoteOrigin};

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
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
        let session =
            stored(&manager, session, &[zeta.clone(), quiet.clone(), alpha.clone()]).await;
        for (channel, text) in [(&zeta, "noted in zeta"), (&alpha, "noted in alpha")] {
            let mut state = AgentChannelState::default();
            state.private_notes.push(
                Change::Add {
                    id: NoteId(1),
                    text: text.to_string(),
                    classification: Classification::CHANNEL_DEFAULT,
                },
                NoteOrigin::default(),
                MessageId::default(),
            );
            store
                .state_store()
                .put_agent_channel_state(helper.id, channel.id, &mut state)
                .await
                .unwrap();
        }

        let read = |viewer: Viewer| {
            let manager = &manager;
            async move {
                manager
                    .views(viewer)
                    .session(session.id)
                    .await
                    .unwrap()
                    .private_notes_of(helper.id)
                    .await
            }
        };
        let found = read(as_user(alice.id)).await.expect("alice made the agent");
        let found: Vec<_> = found
            .iter()
            .map(|(channel, notes, _)| (channel.name().as_str(), notes[0].text.as_str()))
            .collect();
        assert_eq!(
            found,
            [("alpha", "noted in alpha"), ("zeta", "noted in zeta")],
            "by the channels' names, without one noting nothing"
        );
        let refused = read(as_user(bob.id)).await.expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
    }

    #[tokio::test]
    async fn an_agent_whose_personality_is_gone_keeps_its_name_and_its_state() {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::note::{NoteId, NoteOrigin};

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let system = SecurityContext::system();
        let session = stored(&manager, session, slice::from_ref(&channel)).await;
        // What it kept while it ran: the same state, whatever its kind was
        // (issue 206).
        let mut kept = AgentChannelState::default();
        kept.notes.push(
            Change::Add {
                id: NoteId(1),
                text: "kept".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            NoteOrigin::default(),
            MessageId::default(),
        );
        store
            .state_store()
            .put_agent_channel_state(helper.id, channel.id, &mut kept)
            .await
            .unwrap();
        assert!(store
            .entity_stores()
            .unwrap()
            .personalities
            .delete(&system, helper.personality_id, &Removal::now(system.caller_id))
            .await
            .unwrap());

        let for_alice = manager
            .views(as_user(alice.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap();
        let participant = for_alice.participant(ActorId::Agent(helper.id)).await.unwrap();
        assert_eq!(
            (participant.name(), participant.handle().as_deref()),
            (Some("Helper"), Some("helper"))
        );
        assert!(
            participant.may_read_context().await,
            "a question of its document, not of its kind"
        );
        assert_eq!(
            participant.introduction().await.unwrap(),
            None,
            "its personality introduced it"
        );
        let notes = participant.notes().await.unwrap().expect("still an agent");
        assert_eq!(
            notes.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["kept"],
            "what it kept is read"
        );
        assert_eq!(
            for_alice.notes().await.unwrap().len(),
            1,
            "and counts among the channel's notes"
        );
        assert!(participant.context().await.is_ok(), "as what it saw");
    }

    #[tokio::test]
    async fn the_approvals_a_channel_waits_for_are_read_from_its_agents_turns() {
        use caelumora_model::actor::ActorState;
        use caelumora_model::tool::{ToolCall, ToolCallState};
        use caelumora_model::turn::TurnState;

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        // The agent's turn waits for one call, the other was approved.
        let call = |name: &str| ToolCall {
            id: Some(name.to_string()),
            function_name: name.to_string(),
            arguments: serde_json::json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        };
        let waiting = ToolCallState::pending(
            call("delete"),
            "Delete it?".to_string(),
            false,
            Some(ToolMatcher::everything()),
            None,
        );
        let mut turn = TurnState::default();
        turn.work.event_context.session_id = session.id;
        turn.work.event_context.channel_id = channel.id;
        turn.work.event_context.sender_id = ActorId::Agent(helper.id);
        turn.tool_calls = vec![waiting.clone(), ToolCallState::approved(call("echo"))];
        let mut state = AgentChannelState {
            processing_state: ActorState::WaitingForUser,
            turn: Some(turn),
            ..Default::default()
        };
        store
            .state_store()
            .put_agent_channel_state(helper.id, channel.id, &mut state)
            .await
            .unwrap();

        let main = manager
            .views(as_user(alice.id))
            .session(session.id)
            .await
            .unwrap()
            .channel(channel.id)
            .await
            .unwrap();
        let pending = main.pending_approvals().await.unwrap();
        assert_eq!(pending.len(), 1, "only the call that waits");
        assert_eq!(
            (
                pending[0].id,
                pending[0].agent_id,
                pending[0].channel_id,
                pending[0].description.as_str()
            ),
            (waiting.approval_id().unwrap(), helper.id, channel.id, "Delete it?")
        );
        assert!(manager.running(session.id).is_none(), "read without starting the session");
    }

    #[tokio::test]
    async fn a_snapshot_holds_the_details_asked_for_and_no_others() {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId, TokenUsage};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::snapshot::ChannelParticipantSnapshot;
        use caelumora_model::tool::{ToolCall, ToolCallState};
        use caelumora_model::turn::TurnState;

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        let session = stored(&manager, session, slice::from_ref(&channel)).await;
        // What the agent's call used, what it noted, and a call its turn
        // waits to have approved.
        let answer = Message {
            sender_id: ActorId::Agent(helper.id),
            prompt_tokens: Some(1000),
            completion_tokens: Some(50),
            context_size: Some(8192),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let mut state = AgentChannelState {
            used: TokenUsage::counted(None, &answer),
            ..AgentChannelState::default()
        };
        state.transcript.push(answer);
        state.notes.push(
            Change::Add {
                id: NoteId(1),
                text: "noted".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            NoteOrigin::default(),
            MessageId::default(),
        );
        let mut turn = TurnState::default();
        turn.work.event_context.sender_id = ActorId::Agent(helper.id);
        let call = ToolCall {
            id: Some("delete".to_string()),
            function_name: "delete".to_string(),
            arguments: serde_json::json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        };
        turn.tool_calls = vec![ToolCallState::pending(
            call,
            "Delete it?".to_string(),
            false,
            Some(ToolMatcher::everything()),
            None,
        )];
        state.turn = Some(turn);
        store
            .state_store()
            .put_agent_channel_state(helper.id, channel.id, &mut state)
            .await
            .unwrap();

        let views = manager.views(as_user(alice.id));
        let main = views.session(session.id).await.unwrap().channel(channel.id).await.unwrap();
        // As counted: the last call's measure, read as it is stored (issue 390).
        let expected = TokenUsage {
            calls: 1,
            prompt_tokens: 1000,
            completion_tokens: 50,
            context_size: Some(8192),
            context_tokens: 1050,
        };
        let details = |participant: &ChannelParticipantSnapshot| {
            (participant.usage, participant.notes.len(), participant.pending_approvals.len())
        };

        let all = main
            .snapshot(Detail::PARTICIPANTS | Detail::USAGE | Detail::NOTES | Detail::APPROVALS)
            .await;
        let [user, agent] = all.participants.as_slice() else {
            panic!("two participants: {all:?}")
        };
        assert_eq!(details(user), (None, 0, 0), "a user has none of them");
        assert_eq!(details(agent), (Some(expected), 1, 1), "an agent's, from its state");
        let alone = main
            .participant(ActorId::Agent(helper.id))
            .await
            .unwrap()
            .snapshot(Detail::USAGE)
            .await;
        assert_eq!(details(&alone), (Some(expected), 0, 0), "only what was asked for");
        let plain = main.snapshot(Detail::PARTICIPANTS).await;
        assert!(plain.participants.iter().all(|m| details(m) == (None, 0, 0)), "none asked for");
        assert!(
            main.snapshot(Detail::USAGE).await.participants.is_empty(),
            "no participants to carry it"
        );
        let listed = views.session(session.id).await.unwrap().snapshot(Detail::CHANNELS).await;
        assert!(
            !listed.channels.is_empty()
                && listed.channels.iter().all(|c| c.participants.is_empty()),
            "only what was asked for"
        );
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn an_agent_reads_another_channel_as_a_whole_by_their_classifications_or_what_was_released_to_it(
    ) {
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};
        use caelumora_model::content::testing::make_content_ref;
        use caelumora_model::content::ContentRef;
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId, MessageRole};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::privacy::SessionPrivacy;

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let (store, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let classified = |level, compartments: &[&str]| {
            Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap()
        };
        let mut channel_of = |name: &str, classification: Classification| {
            let channel = Channel { classification, ..like(&template, name) };
            session.channels.push(channel.id);
            channel
        };
        let public = channel_of("public", classified(Confidentiality::Public, &[]));
        let team = channel_of("team", classified(Confidentiality::Internal, &[]));
        let secret = channel_of("secret", classified(Confidentiality::Confidential, &[]));
        let hr = channel_of("hr", classified(Confidentiality::Internal, &["hr"]));
        let session =
            stored(&manager, session, &[public.clone(), team.clone(), secret.clone(), hr.clone()])
                .await;

        // Said and noted in secret: one of each released to everyone.
        let said = |t: &str, classification: &Classification| Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(t.to_string()),
            ..Message::new(classification.clone())
        };
        let mut state = store.state_store().get_channel_state(secret.id).await.unwrap();
        let plan = said("the plan", &secret.classification);
        // Attached to what was released, each with its own label (issue 282) - kept nowhere.
        let attached = |name: &str, classification: &Classification| ContentRef {
            classification: classification.clone(),
            storage: "nowhere".to_string(),
            ..make_content_ref(name, b"")
        };
        let agenda = attached("agenda.txt", &public.classification);
        let minutes = attached("minutes.txt", &secret.classification);
        let date = Message {
            attachments: vec![agenda.clone(), minutes.clone()],
            ..said("the date", &public.classification)
        };
        state.history.extend(vec![plan.clone(), date.clone()]);
        let ended = HistoryEntry::TurnGivenUp {
            turn_id: TurnId::new(),
            actor: ActorId::Agent(helper.id),
            reason: GivenUpReason::Left,
            at: Utc::now(),
        };
        state.history.record(ended);
        store.state_store().put_channel_state(secret.id, &mut state).await.unwrap();
        let mut agent_state = AgentChannelState::default();
        let note = |id, text: &str, classification: &Classification| Change::Add {
            id: NoteId(id),
            text: text.to_string(),
            classification: classification.clone(),
        };
        agent_state.notes.push(
            note(1, "noted secretly", &secret.classification),
            NoteOrigin::default(),
            MessageId::default(),
        );
        agent_state.notes.push(
            note(2, "noted openly", &public.classification),
            NoteOrigin::default(),
            MessageId::default(),
        );
        store
            .state_store()
            .put_agent_channel_state(helper.id, secret.id, &mut agent_state)
            .await
            .unwrap();
        let mut state = store.state_store().get_channel_state(team.id).await.unwrap();
        state.history.extend(vec![said("internal", &team.classification)]);
        store.state_store().put_channel_state(team.id, &mut state).await.unwrap();

        // The agent, reading from one of its channels, as its components do.
        let reading_from = |channel: &Channel| {
            let context = SecurityContext::new(ActorId::User(alice.id), session.id, channel.id)
                .next(ActorId::Agent(helper.id), ActorId::User(alice.id), ActorId::User(alice.id));
            let from = Place {
                channel: channel.id,
                classification: channel.classification.clone(),
                session: SessionPrivacy::of(&session),
            };
            Viewer::from_place(context, from)
        };
        let names = |channels: Vec<ChannelView>| {
            channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>()
        };
        let texts = |page: HistoryPage| {
            messages(&page).iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
        };

        let from_team = manager.views(reading_from(&team)).session(session.id).await.unwrap();
        assert_eq!(
            names(from_team.channels().await),
            ["public", "team"],
            "as a whole: what is public, and itself"
        );
        assert_eq!(
            names(from_team.all_channels().await),
            ["public", "team", "secret", "hr"],
            "the rest entry by entry"
        );
        let secret_from_team =
            from_team.channel_named("secret").await.expect("reachable for what was released");
        let released = whole(&secret_from_team).await.unwrap();
        assert_eq!(released.items.len(), 1, "nothing of how turns ended: it would tell of work whose messages are not released (issue 258)");
        assert_eq!(texts(released), ["the date"]);
        let page = secret_from_team.history_page(None, 10).await.unwrap();
        assert_eq!(
            page.entries()
                .filter_map(HistoryEntry::said)
                .filter_map(|m| m.content.as_deref())
                .collect::<Vec<_>>(),
            ["the date"],
            "a page of what was released (issue 218)"
        );
        assert!(
            matches!(secret_from_team.message(plan.id).await, Err(ViewError::NoChannelNamed(_))),
            "what was not released is not found"
        );
        assert!(
            matches!(
                secret_from_team.attachment(date.id, minutes.id).await,
                Err(ViewError::Attachment(AttachmentError::NotFound(_)))
            ),
            "not released by its own label, though its message is"
        );
        assert_eq!(
            secret_from_team.attachment(date.id, agenda.id).await.unwrap(),
            agenda,
            "released"
        );
        assert!(
            matches!(
                secret_from_team.attachment_content(date.id, agenda.id).await,
                Err(ViewError::Attachment(AttachmentError::StorageGone(_)))
            ),
            "and read from its storage"
        );
        let whole_secret = manager
            .views(Viewer::new(SecurityContext::new(
                ActorId::User(alice.id),
                session.id,
                secret.id,
            )))
            .session(session.id)
            .await
            .unwrap()
            .channel(secret.id)
            .await
            .unwrap();
        assert_eq!(
            whole_secret.attachment(date.id, minutes.id).await.unwrap(),
            minutes,
            "read as a whole: every one"
        );
        let notes = secret_from_team.notes().await.unwrap();
        assert_eq!(
            notes[0].notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["noted openly"]
        );
        let refused = secret_from_team
            .participant(ActorId::Agent(helper.id))
            .await
            .unwrap()
            .context()
            .await
            .expect_err("not as a whole");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let hr_from_team = from_team.channel_named("hr").await.unwrap();
        assert!(
            matches!(whole(&hr_from_team).await, Err(ViewError::NoChannelNamed(name)) if name == "hr"),
            "nothing released: as if it did not exist"
        );
        assert!(matches!(hr_from_team.notes().await, Err(ViewError::NoChannelNamed(_))));
        assert!(matches!(
            hr_from_team.history_page(None, 10).await,
            Err(ViewError::NoChannelNamed(_))
        ));
        assert!(matches!(from_team.channel_named("nope").await, Err(ViewError::NoChannelNamed(_))));

        let from_secret = manager.views(reading_from(&secret)).session(session.id).await.unwrap();
        assert_eq!(
            names(from_secret.channels().await),
            ["public", "team", "secret"],
            "confidential reads internal; HR's never a general channel"
        );
        assert_eq!(
            texts(whole(&from_secret.channel_named("team").await.unwrap()).await.unwrap()),
            ["internal"]
        );
        let itself = from_secret.channel(secret.id).await.unwrap();
        let history = whole(&itself).await.unwrap();
        assert_eq!(
            history.items.len(),
            4,
            "its member's join, its messages, and how its turns ended (issue 258)"
        );
        assert_eq!(texts(history), ["the plan", "the date"], "a channel reads itself as a whole");
        assert!(itself
            .participant(ActorId::Agent(helper.id))
            .await
            .unwrap()
            .context()
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_participant_that_left_is_still_reached_by_what_it_said_and_a_gone_channel_is_told_from_one_never_there(
    ) {
        use caelumora_model::message::{Message, MessageId};

        let (store, manager) = owned_runtime(Default::default());
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        // The agent took part, said something, and was removed. Beside it, a
        // channel deleted later.
        let channel = main_channel(&session);
        let deleted = Channel {
            id: ChannelId::new(),
            name: Name::new("deleted").unwrap(),
            ..channel.clone()
        };
        let session = stored(&manager, session, &[channel.clone(), deleted.clone()]).await;
        let mut state = store.state_store().get_channel_state(channel.id).await.unwrap();
        state.history.extend(vec![Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(helper.id),
            content: Some("I was here".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }]);
        store.state_store().put_channel_state(channel.id, &mut state).await.unwrap();

        let views = manager.views(as_user(alice.id));
        let reached =
            views.channel_of(session.id, channel.id).await.unwrap().there().expect("there");
        let former =
            reached.participant(ActorId::Agent(helper.id)).await.expect("known by what it said");
        assert!(former.former() && former.name() == Some("Helper"));
        assert!(former.context().await.is_ok(), "what it saw is still the owner's to read");
        assert!(
            reached.participants().await.iter().all(|m| m.id() != ActorId::Agent(helper.id)),
            "not listed among the participants"
        );
        assert!(
            matches!(
                reached.participant(ActorId::User(UserId::new())).await,
                Err(ViewError::ParticipantNotFound { .. })
            ),
            "a user who never took part"
        );

        // Never there: another channel, another session - nor for a stranger.
        assert!(matches!(
            views.channel_of(session.id, ChannelId::new()).await.unwrap(),
            Reached::Never
        ));
        assert!(matches!(
            views.channel_of(SessionId::new(), channel.id).await.unwrap(),
            Reached::Never
        ));
        let stranger = manager.views(as_user(UserId::new()));
        assert!(
            matches!(stranger.channel_of(session.id, reached.id()).await.unwrap(), Reached::Never),
            "nor is it there for a stranger"
        );

        // Gone (issue 164): the channel deleted, then the session - with who
        // deleted it, to whoever could read it, and never there for others.
        let alice_ctx = as_user(alice.id).context().clone();
        store
            .entity_stores()
            .unwrap()
            .channels
            .delete(&alice_ctx, deleted.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap();
        match views.channel_of(session.id, deleted.id).await.unwrap() {
            Reached::Gone(removal) => assert_eq!(removal.by, ActorId::User(alice.id)),
            other => panic!("gone, not {other:?}"),
        }
        assert!(
            matches!(stranger.channel_of(session.id, deleted.id).await.unwrap(), Reached::Never),
            "not to a stranger"
        );
        store
            .entity_stores()
            .unwrap()
            .sessions
            .delete(&alice_ctx, session.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap();
        assert!(
            matches!(views.channel_of(session.id, channel.id).await.unwrap(), Reached::Gone(_)),
            "gone with its session"
        );
        assert!(matches!(
            stranger.channel_of(session.id, channel.id).await.unwrap(),
            Reached::Never
        ));
    }

    #[tokio::test]
    async fn a_gone_channel_is_reached_as_a_live_one_by_the_clearance_too() {
        use caelumora_model::confidentiality::{Clearance, Confidentiality};
        use caelumora_model::role::{RoleId, Roles};
        let staff = RoleId::new();
        let roles = Roles::new([(
            staff,
            "staff".to_string(),
            vec![Clearance { level: Confidentiality::Internal, compartment: None }],
        )])
        .unwrap();
        let (store, manager) = owned_runtime(ConfidentialityRules::default().with_roles(roles));
        let alice = user("alice");
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        let main = Channel {
            classification: Classification::general(Confidentiality::Internal),
            ..main_channel(&session)
        };
        let internal =
            Channel { id: ChannelId::new(), name: Name::new("side").unwrap(), ..main.clone() };
        let secret = Channel {
            id: ChannelId::new(),
            name: Name::new("secret").unwrap(),
            classification: Classification::general(Confidentiality::Confidential),
            ..main.clone()
        };
        let session =
            stored(&manager, session, &[main.clone(), internal.clone(), secret.clone()]).await;
        store
            .entity_stores()
            .unwrap()
            .channels
            .delete(&SecurityContext::system(), internal.id, &Removal::now(ActorId::System))
            .await
            .unwrap();
        store
            .entity_stores()
            .unwrap()
            .channels
            .delete(&SecurityContext::system(), secret.id, &Removal::now(ActorId::System))
            .await
            .unwrap();

        // Alice owns both, and is cleared for internal alone (issue 304).
        let hers = manager.views(Viewer::new(
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default())
                .with_roles(vec![staff], None),
        ));
        assert!(
            matches!(hers.channel_of(session.id, internal.id).await.unwrap(), Reached::Gone(_)),
            "gone, where she could read it"
        );
        assert!(
            matches!(hers.channel_of(session.id, secret.id).await.unwrap(), Reached::Never),
            "never there, as a live one above her clearance is not found"
        );
    }

    #[tokio::test]
    async fn what_was_deleted_is_shown_to_its_owner_with_what_they_may_do_about_it() {
        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let channel = main_channel(&session);
        let side = like(&channel, "side");
        let session = stored(&manager, session, &[channel.clone(), side.clone()]).await;
        let alice_ctx = as_user(alice.id).context().clone();
        store
            .entity_stores()
            .unwrap()
            .channels
            .delete(&alice_ctx, side.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap();

        let hers = manager.views(as_user(alice.id));
        let channels = hers.session(session.id).await.unwrap().deleted_channels().await.unwrap();
        assert_eq!(
            channels.iter().map(|c| (c.id, c.name.as_str())).collect::<Vec<_>>(),
            [(side.id, "side")]
        );
        assert!(
            channels[0].can_restore && !channels[0].can_purge,
            "restored by her, purged only with the session"
        );
        assert_eq!(channels[0].deleted.by, ActorId::User(alice.id));
        let his = manager.views(as_user(bob.id));
        assert!(
            his.session(session.id).await.unwrap().deleted_channels().await.unwrap().is_empty(),
            "not shown to a contributor"
        );

        store
            .entity_stores()
            .unwrap()
            .sessions
            .delete(&alice_ctx, session.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap();
        let sessions = hers.deleted_sessions().await.unwrap();
        assert_eq!(sessions.iter().map(|s| s.id).collect::<Vec<_>>(), [session.id]);
        assert!(sessions[0].can_restore && !sessions[0].can_purge, "purging is not the owner's");
        assert!(his.deleted_sessions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn who_left_is_a_former_participant_with_who_removed_them_also_without_a_word() {
        use caelumora_model::channel::FormerParticipant;
        use caelumora_model::removal::Removal;

        let (store, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        // Both took part and were removed by alice, neither said anything.
        let mut channel = main_channel(&session);
        channel.participants.retain(|p| {
            p.actor_id() != ActorId::User(bob.id) && p.actor_id() != ActorId::Agent(helper.id)
        });
        let removed = Removal::now(ActorId::User(alice.id));
        channel.former = vec![
            FormerParticipant { participant: Participant::User(bob.id), left: removed.clone() },
            FormerParticipant { participant: Participant::Agent(helper.id), left: removed.clone() },
        ];
        let session = stored(&manager, session, slice::from_ref(&channel)).await;

        let reached = manager
            .views(as_user(alice.id))
            .channel_of(session.id, channel.id)
            .await
            .unwrap()
            .there()
            .expect("there");
        let former =
            reached.participant(ActorId::Agent(helper.id)).await.expect("kept as having left");
        assert!(former.former() && former.left() == Some(&removed));
        assert!(former.context().await.is_ok(), "what it saw is still the owner's to read");
        let bob_left = reached.participant(ActorId::User(bob.id)).await.expect("a user too");
        assert!(bob_left.former() && bob_left.name() == Some("bob"));
        assert_eq!(
            reached.former_participants().await.iter().map(ParticipantView::id).collect::<Vec<_>>(),
            [ActorId::User(bob.id), ActorId::Agent(helper.id)],
            "in the order they left"
        );
        assert!(
            reached.participants().await.iter().all(|m| !m.former()),
            "not among the participants"
        );

        let snapshot = reached.snapshot(Detail::PARTICIPANTS).await;
        assert_eq!(snapshot.former_participants.len(), 2);
        assert!(
            snapshot.former_participants.iter().all(|m| m.left.as_ref() == Some(&removed)),
            "each with who removed it, and when"
        );
        assert!(snapshot.participants.iter().all(|m| m.left.is_none()));
    }

    #[tokio::test]
    async fn a_channel_reads_another_by_their_classification_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let runtime = Arc::clone(manager.runtime());

        let (alice, bob) = (user("alice"), user("bob"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        // Her context carries her roles, as her controller gives it (issue 304).
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default())
            .with_roles(alice.roles.clone(), None);
        let mut session = make_session("hers", alice.id);
        session.security_attributes =
            session.security_attributes.clone().granting(Subject::Actor(bob_id), Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| {
            Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap()
        };
        let with = |name: &str, classification, participants| ChannelSpec {
            classification: Some(classification),
            ..spec(name, participants)
        };
        let public = session
            .create_channel(
                &alice_ctx,
                with("public", classified(Confidentiality::Public, &[]), vec![bob_id]),
            )
            .await
            .unwrap();
        let team = session
            .create_channel(
                &alice_ctx,
                with("team", classified(Confidentiality::Internal, &[]), vec![bob_id]),
            )
            .await
            .unwrap();
        let secret = session
            .create_channel(
                &alice_ctx,
                with("secret", classified(Confidentiality::Confidential, &[]), vec![]),
            )
            .await
            .unwrap();
        let hr = session
            .create_channel(
                &alice_ctx,
                with("hr", classified(Confidentiality::Internal, &["hr"]), vec![]),
            )
            .await
            .unwrap();

        let confidentiality = runtime.confidentiality();
        let may_read = |reader: &Arc<ChannelLoop>, source: &Arc<ChannelLoop>| {
            let reader = Arc::clone(reader);
            let source = Arc::clone(source);
            let confidentiality = Arc::clone(confidentiality);
            let context = alice_ctx.clone();
            async move {
                use caelumora_model::confidentiality::{ConfidentialityQuestions, Place};
                let place = |channel: &ChannelLoop| Place {
                    channel: channel.id(),
                    classification: channel.classification(),
                    session: channel.session_loop().map(|s| s.privacy()).unwrap_or_default(),
                };
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

        let refused = session
            .create_channel(
                &alice_ctx,
                with("typo", classified(Confidentiality::Internal, &["hrr"]), vec![]),
            )
            .await
            .expect_err("not configured");
        assert!(
            matches!(
                refused.downcast_ref::<SessionError>(),
                Some(SessionError::ClassificationRefused(_))
            ),
            "{refused}"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_reader_knows_the_privacy_of_its_session_and_looks_up_others() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;

        let (_, manager) = owned_runtime(Default::default());
        let (alice, carol) = (user("alice"), user("carol"));
        let (alice_id, carol_id) = (ActorId::User(alice.id), ActorId::User(carol.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());

        let mut hers = make_session("hers", alice.id);
        hers.participants = record_users(&manager, &[&alice]).await;
        hers.privacy = PrivacyConfig::SameOwner;
        let hers = create(&manager, &alice_ctx, hers).await.unwrap();
        let theirs = make_session("theirs", carol.id);
        persist(
            &manager,
            &SecurityContext::new(carol_id, Default::default(), Default::default()),
            theirs.clone(),
            &[main_channel(&theirs)],
        )
        .await;

        let channel = Arc::clone(&hers.channels()[0]);
        let runtime = actor_runtime_in(&channel, alice_id);
        let session = runtime
            .session_view(&alice_ctx, channel.classification())
            .await
            .expect("hers to read")
            .expect("attached to its session");
        let stranger = SecurityContext::new(
            ActorId::Agent(AgentId::new()),
            Default::default(),
            Default::default(),
        );
        assert!(
            runtime.session_view(&stranger, channel.classification()).await.is_err(),
            "reached by who may read it alone (issue 304)"
        );

        let own = session.privacy();
        assert_eq!((own.policy, own.owner), (PrivacyConfig::SameOwner, alice_id));
        assert_eq!(own.people, [alice.id].into_iter().collect(), "its users and its owner");
        let other =
            session.views().privacy_of(theirs.id).await.unwrap().expect("stored, running or not");
        assert_eq!(
            (other.policy, other.owner),
            (PrivacyConfig::Private, carol_id),
            "even a session the reader may not read"
        );
        assert!(
            session.views().privacy_of(SessionId::new()).await.unwrap().is_none(),
            "one that is gone"
        );
        manager.stop(&SecurityContext::system(), hers.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_released_message_is_read_where_its_channel_is_not() {
        use crate::view::ViewError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};

        let compartments = Compartments::new([
            (Compartment::new("hr"), vec![]),
            (Compartment::new("finance"), vec![]),
        ])
        .unwrap();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments));
        let alice = user("alice");
        let alice_id = ActorId::User(alice.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| {
            Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap()
        };
        let with = |name: &str, classification| ChannelSpec {
            classification: Some(classification),
            ..spec(name, vec![])
        };
        let hr = session
            .create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"])))
            .await
            .unwrap();
        let finance = session
            .create_channel(
                &alice_ctx,
                with("finance", classified(Confidentiality::Internal, &["finance"])),
            )
            .await
            .unwrap();
        let team = session
            .create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[])))
            .await
            .unwrap();

        let writer = hr.user_actor(alice.id).expect("the creator takes part");
        let mut events = writer.watch();
        writer.send_message(Some("for HR".to_string()), None, Vec::new()).await.unwrap();
        writer
            .send_message(
                Some("for finance too".to_string()),
                Some(classified(Confidentiality::Internal, &["hr", "finance"])),
                Vec::new(),
            )
            .await
            .unwrap();
        writer
            .send_message(
                Some("too secret".to_string()),
                Some(classified(Confidentiality::Confidential, &["hr"])),
                Vec::new(),
            )
            .await
            .expect("sent: the channel decides");
        assert_eq!(
            rejection(&mut events).await.1,
            Rejection::ClassificationRefused,
            "above the channel, and its writer is told (issue 304)"
        );
        let history = history_of(&hr, 2).await;
        assert_eq!(
            history[0].classification,
            classified(Confidentiality::Internal, &["hr"]),
            "the channel's, unless classified otherwise"
        );

        // Read as an agent in another channel reads: through its actor's runtime.
        let reading_from = |channel: &Arc<ChannelLoop>| {
            let runtime = actor_runtime_in(channel, alice_id);
            let (context, classification) = (alice_ctx.clone(), channel.classification());
            async move {
                runtime
                    .session_view(&context, classification)
                    .await
                    .expect("hers to read")
                    .expect("attached to its session")
            }
        };
        let read = whole(
            &reading_from(&finance)
                .await
                .channel_named("hr")
                .await
                .expect("reachable for what was released"),
        )
        .await
        .expect("what was released to finance");
        let texts: Vec<_> =
            messages(&read).iter().map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(texts, ["for finance too"], "only what was released");
        let nothing =
            whole(&reading_from(&team).await.channel_named("hr").await.expect("reachable, but"))
                .await;
        assert!(
            matches!(nothing, Err(ViewError::NoChannelNamed(_))),
            "nothing was released to a general channel"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_participant_s_answer_under_way_is_in_the_snapshot_of_its_channel() {
        use caelumora_model::actor::{ActorEvent, ActorId};

        let store = Persistence::in_memory();
        let runtime = TestRuntime {
            store: store.clone(),
            plugins: vec![&ScriptedAgents(Script::Hangs)],
            ..Default::default()
        }
        .build();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();

        let alice = user("alice");
        let participants = record_users(&manager, &[&alice]).await;
        let personality = make_personality("thinker");
        store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
        let thinker = Agent::new(
            ScopedName::new("thinker", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(alice.id),
        );
        store.entity_stores().unwrap().agents.create(&system, &thinker).await.unwrap();
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking", UserId::new());
        session.participants = participants;
        session.participants.push(SessionParticipant::Agent(thinker.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session
            .create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), agent]))
            .await
            .unwrap();
        let mut events = channel.user_actor(alice.id).unwrap().watch();
        let shown = |events: &mut ChannelWatch, wanted: &'static str| {
            let mut events = events.resubscribe();
            async move {
                time::timeout(Duration::from_secs(5), async {
                    loop {
                        let envelope = events.recv().await.expect("the channel is running");
                        if envelope.context.sender_id == agent && envelope.event.kind() == wanted {
                            break envelope.event;
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("the agent sends {wanted}"))
            }
        };
        let participant = || async {
            let view = manager
                .views(Viewer::new(alice_ctx.clone()))
                .session(session.id())
                .await
                .unwrap()
                .channel(channel.id())
                .await
                .unwrap();
            view.snapshot(Detail::PARTICIPANTS)
                .await
                .participants
                .into_iter()
                .find(|m| m.id == agent)
                .expect("a participant")
        };
        assert_eq!(participant().await.turn_under_way, None, "nothing under way yet");

        let started = shown(&mut events, "ResponseStarted");
        channel
            .user_actor(alice.id)
            .unwrap()
            .send_message(Some("hello".to_string()), None, Vec::new())
            .await
            .unwrap();
        let ActorEvent::ResponseStarted { turn_id } = started.await else { unreachable!() };
        assert_eq!(
            participant().await.turn_under_way,
            Some(turn_id),
            "the turn its start announced"
        );

        let ended = shown(&mut events, "TurnFinished");
        channel.cancel_actor(&alice_ctx, agent).await.unwrap();
        ended.await;
        assert_eq!(participant().await.turn_under_way, None, "over once it ended");
        manager.stop(&system, session.id()).await.unwrap();
    }
}
