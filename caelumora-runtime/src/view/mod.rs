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
//! their classifications (`AccessQuestions::may_copy_information`, issue 133),
//! else entry by entry - only what was released to it - and a channel it
//! may not read at all is one that does not exist. Its root is its own
//! session, reached through its turn (`Turn::session`); a frontend's root
//! is [`SessionManager::views`].
//!
//! A node answers with a snapshot (`caelumora_model::snapshot`, issue 80)
//! for a client that lists and offers: the node's data with its questions
//! answered at one moment.

mod artifact;
mod channel;
mod context;
mod member;
mod session;
mod skill;
mod workspace;

pub use artifact::ArtifactView;
pub use channel::{ChannelView, MemberView};
pub use context::{AgentContextView, ContextError};
pub use session::{PrivateNotesElsewhere, SessionView};
pub use skill::SkillView;
pub use workspace::WorkspaceView;

use caelumora_model::store::query::Query;
use crate::runtime::{ActorRuntime, RuntimeManager};
use crate::session::{Found, SessionLoop, SessionManager};
use crate::tool::ToolError;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::confidentiality::Classification;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::removal::Removal;
use caelumora_model::security::{AccessQuestions, Place, SecurityContext, SecurityManager};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::{AgentSnapshot, DeletedSnapshot};
use caelumora_model::workspace::WorkspaceId;
use caelumora_model::skill::SkillId;
use caelumora_model::scope::{found_from, Scope, Scoped};
use caelumora_model::value::name::Name;
use crate::workspace::WorkspaceManager;
use crate::skill::SkillManager;
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
    #[error("There is no skill {0}")]
    SkillNotFound(SkillId),
    /// What an agent asks for by name: no skill of the name it may load
    /// where it is, alike whether there is one at all (issue 22).
    #[error("there is no skill named \"{0}\" that can be loaded here")]
    NoSkillNamed(String),
    #[error("There is no artifact {0}")]
    ArtifactNotFound(caelumora_model::artifact::ArtifactId),
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
    workspaces: Arc<WorkspaceManager>,
    skills: Arc<SkillManager>,
    artifacts: Arc<crate::artifact::ArtifactManager>,
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
            workspaces: Arc::clone(runtime.workspace_manager()),
            skills: Arc::clone(runtime.skill_manager()),
            artifacts: Arc::clone(runtime.artifact_manager()),
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

    pub(crate) fn workspace_manager(&self) -> &Arc<WorkspaceManager> {
        &self.workspaces
    }

    /// Where an agent's tools write skills (issue 22), as its file tools
    /// reach the workspace manager.
    pub(crate) fn skill_manager(&self) -> &Arc<SkillManager> {
        &self.skills
    }

    pub(crate) fn artifact_manager(&self) -> &Arc<crate::artifact::ArtifactManager> {
        &self.artifacts
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        let sessions = self.entities.list_sessions(self.viewer.context(), &Query::all()).await.map(|page| page.items)
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
        let agents = self.entities.list_agents(self.viewer.context(), &Query::all()).await.map(|page| page.items)
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
        let agents = self.entities.list_deleted_agents(self.viewer.context(), &Query::all()).await.map(|page| page.items)
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

    /// The workspaces the viewer may read (issue 23) - their own and those
    /// shared with them, wherever they lie - and is cleared for, in the
    /// order the store lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn workspaces(&self) -> ViewResult<Vec<WorkspaceView>> {
        let workspaces = self.entities.list_workspaces(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the workspaces")))?;
        let mut readable = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            if self.cleared_for(&workspace.classification).await {
                readable.push(WorkspaceView::direct(self.clone(), workspace));
            }
        }
        Ok(readable)
    }

    /// Workspace `id`, if the viewer may read it (`GetWorkspace`, issue 23)
    /// and is cleared for it.
    ///
    /// # Errors
    /// `WorkspaceNotFound` if there is none they may read - one they are
    /// not cleared for is the same.
    pub async fn workspace(&self, id: WorkspaceId) -> ViewResult<WorkspaceView> {
        match self.entities.get_workspace(self.viewer.context(), id).await.optional() {
            Ok(Some(workspace)) if self.cleared_for(&workspace.classification).await => Ok(WorkspaceView::direct(self.clone(), workspace)),
            Ok(Some(_)) => Err(ViewError::WorkspaceNotFound(id)),
            Ok(None) => Err(ViewError::WorkspaceNotFound(id)),
            Err(e) => Err(ViewError::Failed(anyhow::Error::new(e).context("reading a workspace"))),
        }
    }

    /// The viewer's artifacts (issues 271, 23, 293): those they own and
    /// those shared with them by name - an entry naming them or one of their
    /// roles, in the artifact's list or a version's -, each with the
    /// versions they may read, if they are cleared for it. What they read
    /// only as a member of a channel is found through that channel
    /// (`ChannelView::artifacts`). The store is asked for these alone, in
    /// the runtime's name - its own check knows the artifact's list, not
    /// those of its versions -, and each is asked of the policy here.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn artifacts(&self) -> ViewResult<Vec<ArtifactView>> {
        use caelumora_model::artifact::ArtifactField;
        use caelumora_model::security::Subject;
        use caelumora_model::store::query::Filter;
        let failed = |e: caelumora_model::store::StoreError| ViewError::Failed(anyhow::Error::new(e).context("listing the artifacts"));
        let principal = self.viewer.context().principal_id;
        let roles = caelumora_model::role::user_roles(self.entities.as_ref(), principal).await.map_err(failed)?;
        let subjects: Vec<Subject> = std::iter::once(Subject::Actor(principal)).chain(roles.into_iter().map(Subject::Role)).collect();
        let named = Filter::is_in(ArtifactField::Subject, &subjects).map_err(failed)?;
        let theirs = Filter::Or(vec![
            Filter::eq(ArtifactField::Owner, &principal).map_err(failed)?,
            Filter::any(ArtifactField::Acl, named.clone()),
            Filter::any(ArtifactField::Versions, Filter::any(ArtifactField::VersionAcl, named)),
        ]);
        let artifacts = self.entities.list_artifacts(&SecurityContext::system(), &Query::filtered(theirs)).await.map_err(failed)?.items;
        let mut readable = Vec::new();
        for artifact in artifacts {
            if let Some(view) = self.readable_artifact(artifact).await {
                readable.push(view);
            }
        }
        Ok(readable)
    }

    /// Artifact `id`, with the versions the viewer may read, if they may
    /// read any and are cleared for it (issues 271, 23).
    ///
    /// # Errors
    /// `ArtifactNotFound` if they may read nothing of it - one they are not
    /// cleared for is the same -; `Failed` if the store cannot read it.
    pub async fn artifact(&self, id: caelumora_model::artifact::ArtifactId) -> ViewResult<ArtifactView> {
        let found = self.entities.get_artifact(&SecurityContext::system(), id).await.optional()
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("reading an artifact")))?;
        match found {
            Some(artifact) => self.readable_artifact(artifact).await.ok_or(ViewError::ArtifactNotFound(id)),
            None => Err(ViewError::ArtifactNotFound(id)),
        }
    }

    /// `artifact` with the versions the viewer may read - all of them where
    /// the artifact's own list grants it, otherwise those whose own list
    /// does (issue 23) -, if any and if they are cleared for it.
    pub(super) async fn readable_artifact(&self, mut artifact: caelumora_model::artifact::Artifact) -> Option<ArtifactView> {
        let context = self.viewer.context();
        if !self.security.may_read_artifact(context, &artifact.security_attributes).await {
            let mut readable = Vec::with_capacity(artifact.versions.len());
            for version in std::mem::take(&mut artifact.versions) {
                if self.security.may_read_artifact(context, &artifact.version_attributes(&version)).await {
                    readable.push(version);
                }
            }
            artifact.versions = readable;
        }
        if artifact.versions.is_empty() || !self.cleared_for(&artifact.classification).await {
            return None;
        }
        Some(ArtifactView::of(self.clone(), artifact))
    }

    /// The entries `acl` with whom each names called by name (issue 23),
    /// read in the runtime's name: shown to the owner of what they are of,
    /// who may name a session or user they cannot read themselves.
    pub(crate) async fn access_snapshots(&self, acl: &[caelumora_model::security::AclEntry]) -> Vec<caelumora_model::snapshot::AccessSnapshot> {
        use caelumora_model::security::{Membership, Subject};
        let system = SecurityContext::system();
        let mut access = Vec::with_capacity(acl.len());
        for entry in acl {
            let name = match entry.subject {
                Subject::Actor(ActorId::User(user)) => self.entities.get_user(&system, user).await.optional().ok().flatten().map(|u| u.username.to_string()),
                Subject::Actor(_) => None,
                Subject::Role(role) => self.security.role_name(role),
                Subject::MemberOf(Membership::Session(session)) => self.entities.get_session(&system, session).await.optional().ok().flatten().map(|s| s.name.to_string()),
                Subject::MemberOf(Membership::Channel(channel)) => self.entities.get_channel(&system, channel).await.optional().ok().flatten().map(|c| c.name.to_string()),
            };
            access.push(caelumora_model::snapshot::AccessSnapshot { subject: entry.subject, grant: entry.grant, name });
        }
        access
    }

    /// The artifacts the viewer deleted and may read as such (issue 271):
    /// the trash of artifacts.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_artifacts(&self) -> ViewResult<Vec<DeletedSnapshot<caelumora_model::artifact::ArtifactId>>> {
        let artifacts = self.entities.list_deleted_artifacts(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted artifacts")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let Some(removal) = artifact.deleted.clone() else { continue };
            let attributes = &artifact.security_attributes;
            deleted.push(DeletedSnapshot {
                id: artifact.id,
                name: artifact.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Artifact, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Artifact, attributes).await,
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
        let workspaces = self.entities.list_deleted_workspaces(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted workspaces")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            let Some(removal) = workspace.deleted.clone() else { continue };
            let attributes = &workspace.security_attributes;
            deleted.push(DeletedSnapshot {
                id: workspace.id,
                name: workspace.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Workspace, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Workspace, attributes).await,
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
        let sessions = self.entities.list_deleted_sessions(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted sessions")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(sessions.len());
        for session in sessions {
            let Some(removal) = session.deleted.clone() else { continue };
            let attributes = &session.security_attributes;
            deleted.push(DeletedSnapshot {
                id: session.id,
                name: session.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Session, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Session, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// Whether what `from` holds may be copied into the viewer's place
    /// (`AccessQuestions::may_copy_information`, issues 133, 264). A viewer
    /// without a place reads by other questions and is not asked this one.
    pub(crate) async fn may_copy_from(&self, from: &Place) -> bool {
        let Some(into) = &self.viewer.from else { return true };
        self.security.may_copy_information(self.viewer.context(), from, into).await
    }

    /// The name of `actor`; `None` for a user or agent that no longer
    /// exists. Read in the runtime's name: taking part in a session makes a
    /// name known to everybody in it - it stands at every message - even
    /// where the record itself is not theirs to read (issue 120); so does
    /// owning an agent there, which acts with its owner's authority (issue
    /// 244).
    async fn name_of(&self, actor: ActorId) -> Option<String> {
        match actor {
            ActorId::User(id) => self.entities.get_user(&SecurityContext::system(), id).await.ok().map(|user| user.username.to_string()),
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
    async fn roles(&self) -> Vec<caelumora_model::role::RoleId> {
        caelumora_model::role::user_roles(self.entities.as_ref(), self.viewer.context().principal_id).await.unwrap_or_default()
    }

    /// Whether the viewer is cleared for `classification` (issue 133), by
    /// their user's roles: what one is not cleared for is the same as what
    /// does not exist - a channel, a skill.
    pub(super) async fn cleared_for(&self, classification: &Classification) -> bool {
        let roles = self.roles().await;
        self.security.may_see_channel(self.viewer.context(), &roles, classification).await
    }

    /// The skills the viewer may read (`GetSkill`) and is cleared for (issue
    /// 22): their own, those shared with them, the system's.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn skills(&self) -> ViewResult<Vec<SkillView>> {
        let skills = self.entities.list_skills(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the skills")))?;
        let mut readable = Vec::with_capacity(skills.len());
        for skill in skills {
            if self.cleared_for(&skill.classification).await {
                readable.push(SkillView::of(self.clone(), skill));
            }
        }
        Ok(readable)
    }

    /// Skill `id`, if the viewer may read it and is cleared for it.
    ///
    /// # Errors
    /// `SkillNotFound` if there is none they may read, alike.
    pub async fn skill(&self, id: SkillId) -> ViewResult<SkillView> {
        let skill = match self.entities.get_skill(self.viewer.context(), id).await.optional() {
            Ok(Some(skill)) => skill,
            Ok(None) => return Err(ViewError::SkillNotFound(id)),
            Err(e) => return Err(ViewError::Failed(anyhow::Error::new(e).context("reading a skill"))),
        };
        match self.cleared_for(&skill.classification).await {
            true => Ok(SkillView::of(self.clone(), skill)),
            false => Err(ViewError::SkillNotFound(id)),
        }
    }

    /// The skills the viewer deleted and may read as such: the trash of
    /// skills.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_skills(&self) -> ViewResult<Vec<DeletedSnapshot<SkillId>>> {
        let skills = self.entities.list_deleted_skills(self.viewer.context(), &Query::all()).await.map(|page| page.items)
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the deleted skills")))?;
        let policy = &self.security;
        let mut deleted = Vec::with_capacity(skills.len());
        for skill in skills {
            let Some(removal) = skill.deleted.clone() else { continue };
            let attributes = &skill.security_attributes;
            deleted.push(DeletedSnapshot {
                id: skill.id,
                name: skill.name.to_string(),
                deleted: removal,
                can_restore: policy.may_restore(self.viewer.context(), EntityKind::Skill, attributes).await,
                can_purge: policy.may_purge(self.viewer.context(), EntityKind::Skill, attributes).await,
            });
        }
        Ok(deleted)
    }

    /// The skills an agent may load where it reads from (issue 22): of those
    /// it may read, each it may use (`UseSkill`) whose classification it may
    /// put into its channel (`Publish`, as what a turn writes is asked) -
    /// and of those, for every name the one found from its channel (the
    /// innermost: the session's before its owner's before the global one).
    /// By name. None for a viewer who reads from no place: skills are loaded
    /// into turns.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn usable_skills(&self) -> ViewResult<Vec<SkillView>> {
        let Some(place) = self.viewer.place() else { return Ok(Vec::new()) };
        let context = self.viewer.context();
        let failed = |e: caelumora_model::store::StoreError| ViewError::Failed(anyhow::Error::new(e).context("listing the skills"));
        let here = Scope::ChannelScope { session_id: place.session.id, channel_id: place.channel };
        let owner = context.principal_id.as_user_id();
        // Those seen from here, which the store is asked for alone (issue 293).
        let seen = Query::filtered(Scope::seen_from(&here, owner).map_err(failed)?);
        let skills = self.entities.list_skills(context, &seen).await.map_err(failed)?.items;
        let mut usable = Vec::with_capacity(skills.len());
        for skill in skills {
            if self.security.may_use_skill(context, &skill.security_attributes).await
                && self.security.may_publish(context, context.caller_id, &skill.classification, &place.classification).await
            {
                usable.push(skill);
            }
        }
        Ok(found_from(&usable, &here, owner).into_iter().map(|skill| SkillView::of(self.clone(), skill.clone())).collect())
    }

    /// The skill of `name` the agent may load where it reads from
    /// (`usable_skills`).
    ///
    /// # Errors
    /// `NoSkillNamed` if there is none, and equally if it may not be used
    /// here; `Failed` if the store cannot list them.
    pub async fn usable_skill_named(&self, name: &str) -> ViewResult<SkillView> {
        let not_found = || ViewError::NoSkillNamed(name.to_string());
        let wanted = Name::new(name).map_err(|_| not_found())?;
        self.usable_skills().await?.into_iter().find(|skill| skill.scoped_name().name == wanted).ok_or_else(not_found)
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
/// where its session is reached. Who reads, from which channel and at which
/// classification is the work's, given when the views are built - one
/// context per piece of work (issue 255).
#[derive(Clone)]
pub struct ViewsFactory {
    root: FactoryRoot,
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
    pub(crate) fn of_actor(runtime: ActorRuntime) -> Self {
        Self { root: FactoryRoot::Actor(runtime) }
    }

    #[cfg(test)]
    pub(crate) fn in_session(session: Arc<SessionLoop>) -> Self {
        Self { root: FactoryRoot::Session(session) }
    }

    /// The agent's own session, as it reads in `context` from `channel` at
    /// `classification`; `None` while the channel is not attached to its
    /// session yet, or once it is gone.
    pub(crate) fn session(&self, context: &SecurityContext, channel: ChannelId, classification: Classification) -> Option<SessionView> {
        let session_loop = match &self.root {
            FactoryRoot::Actor(actor) => actor.channel_loop()?.session_loop()?,
            #[cfg(test)]
            FactoryRoot::Session(session) => Arc::clone(session),
        };
        let from = Place { channel, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(context.clone(), from));
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
    use caelumora_model::snapshot::Detail;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::value::text::{Briefing, Summary};
    use crate::runtime::RuntimeManager;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::confidentiality::{Classification, ConfidentialityRules, Visibility};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{Grant, Membership, SecurityAttributes, Subject};
    use caelumora_model::session::Session;
    use caelumora_model::store::EntityStore;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::{User, UserId};

    /// A runtime under the ownership policy, and a manager on it.
    fn owned_runtime() -> (Arc<MemoryStore>, Arc<SessionManager>) {
        owned_runtime_under(ConfidentialityRules::default())
    }

    /// The runtime under the ownership policy with `rules`, which learns who
    /// takes part where from its store (issue 23).
    fn owned_runtime_under(rules: ConfidentialityRules) -> (Arc<MemoryStore>, Arc<SessionManager>) {
        let (policy, store) = crate::session::testing::owned_store(rules);
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        (store, Arc::new(SessionManager::new(runtime)))
    }

    fn user(name: &str) -> User {
        User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None }
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

    /// A session of `owner`, read by its members as one the runtime builds
    /// (issue 23); its participants are the test's to set.
    fn make_session(name: &str, owner: ActorId) -> Session {
        let id = SessionId::new();
        Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id,
            name: Name::new(name).unwrap(),
            participants: Vec::new(),
            channels: vec![ChannelId::new()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::owned_by(owner).granting(Subject::MemberOf(Membership::Session(id)), Grant::Read),
            deleted: None,
        }
    }

    fn main_channel(session: &Session) -> Channel {
        Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            id: session.channels[0],
            session_id: session.id,
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            participants: session.participants.clone(),
            security_attributes: SecurityAttributes::owned_by(session.security_attributes.owner_id).granting(Subject::MemberOf(Membership::Channel(session.channels[0])), Grant::Read),
            deleted: None,
            former: Vec::new(),
        }
    }

    /// Another channel like `template`, named `name`: an id of its own,
    /// read by its own members.
    fn like(template: &Channel, name: &str) -> Channel {
        let id = ChannelId::new();
        let security_attributes = SecurityAttributes::owned_by(template.security_attributes.owner_id).granting(Subject::MemberOf(Membership::Channel(id)), Grant::Read);
        Channel { id, name: Name::new(name).unwrap(), security_attributes, ..template.clone() }
    }

    async fn an_agent_of(store: &MemoryStore, owner: ActorId) -> Agent {
        an_agent_of_kind(store, owner, Default::default()).await
    }

    async fn an_agent_of_kind(store: &MemoryStore, owner: ActorId, kind: caelumora_model::agent::PersonalityKind) -> Agent {
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
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(remote.id));
        let channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();

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
        let (store, manager) = owned_runtime();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;

        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice, &bob]).await;
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

        let (store, manager) = owned_runtime();
        let alice = User { id: UserId::new(), username: Name::new("Alice Example").unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut session = make_session("s", ActorId::User(alice.id));
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
    async fn what_a_place_is_for_is_shown_its_briefing_only_where_asked_for() {
        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice]).await;
        (session.summary, session.briefing) = (Some(Summary::new("Release 2.0").unwrap()), Some(Briefing::new("Dates are in UTC.").unwrap()));
        let mut channel = main_channel(&session);
        (channel.summary, channel.briefing) = (Some(Summary::new("Planning").unwrap()), Some(Briefing::new("Be brief.").unwrap()));
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap();
        let listed = view.snapshot(Detail::CHANNELS).await;
        assert_eq!((listed.summary.as_ref().map(Summary::as_str), listed.briefing.as_ref()), (Some("Release 2.0"), None), "the summary always, the briefing not where listed");
        assert_eq!((listed.channels[0].summary.as_ref().map(Summary::as_str), listed.channels[0].briefing.as_ref()), (Some("Planning"), None));
        let read = view.snapshot(Detail::CHANNELS | Detail::BRIEFING).await;
        assert_eq!((read.briefing.as_ref().map(Briefing::as_str), read.channels[0].briefing.as_ref().map(Briefing::as_str)), (Some("Dates are in UTC."), Some("Be brief.")));
    }

    #[tokio::test]
    async fn a_members_profile_says_who_it_is_to_everybody_in_the_channel() {
        use caelumora_model::agent::PersonalityKind;
        use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
        use caelumora_model::user::UserProfile;

        let (store, manager) = owned_runtime();
        let system = SecurityContext::system();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        // Carol owns the agent but takes no part in the session.
        store.create_user(&system, &carol).await.unwrap();
        let remote = an_agent_of_kind(&store, ActorId::User(carol.id), PersonalityKind::External).await;
        let mut personality = store.get_personality(&system, remote.personality_id).await.unwrap();
        personality.introduction = Some(caelumora_model::value::text::Introduction::new("Answers questions about the weather.").unwrap());
        store.update_personality(&system, &personality).await.unwrap();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };

        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(remote.id));
        let mut channel = main_channel(&session);
        channel.participants.push(Participant::Approver(approver));
        store.create_user_profile(&system, &UserProfile::new(alice.id, Some(caelumora_model::value::text::Introduction::new("I run the project.").unwrap()))).await.unwrap();
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();

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

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
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
        assert!(matches!(seen.context_of(first.id), Err(ContextError::NotRecorded { .. })), "the agent did not produce it");
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

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(bob.id)).await;
        let mut session = make_session("shared", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice, &bob]).await;
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
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        for channel in [&team, &hers] {
            store.create_channel(&system, channel).await.unwrap();
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
    async fn an_agent_loads_the_skills_its_owner_may_use_that_flow_into_its_channel_the_innermost_of_each_name() {
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::skill::Skill;
        use caelumora_model::value::text::{Description, SkillBody};

        let (store, manager) = owned_runtime();
        let (alice, bob) = (UserId::new(), UserId::new());
        let helper = an_agent_of(&store, ActorId::User(bob)).await;
        let (session_id, channel) = (SessionId::new(), ChannelId::new());
        let skill = |name: &str, scope: Scope, owner: ActorId, classification: Classification| Skill::new(
            ScopedName::new(name, scope).unwrap(), Description::new(format!("{name}, described")).unwrap(),
            SkillBody::new(format!("{name}: how it is done")).unwrap(), classification, owner,
        );
        let system = SecurityContext::system();
        let secret = Classification::general(Confidentiality::StrictlyConfidential);
        for made in [
            skill("notes", Scope::GlobalScope, ActorId::System, Classification::CHANNEL_DEFAULT),
            skill("notes", Scope::SessionScope { session_id }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
            skill("deploy", Scope::UserScope { user_id: bob }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
            skill("secret", Scope::UserScope { user_id: bob }, ActorId::User(bob), secret),
            skill("hers", Scope::GlobalScope, ActorId::User(alice), Classification::CHANNEL_DEFAULT),
            Skill { security_attributes: SecurityAttributes::owned_by(ActorId::User(alice)).granting(caelumora_model::security::Subject::Actor(ActorId::User(bob)), caelumora_model::security::Grant::Read), ..skill("review", Scope::GlobalScope, ActorId::User(alice), Classification::CHANNEL_DEFAULT) },
            skill("elsewhere", Scope::SessionScope { session_id: SessionId::new() }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
        ] {
            store.create_skill(&system, &made).await.unwrap();
        }

        let context = SecurityContext::new(ActorId::User(bob), session_id, channel).next(ActorId::Agent(helper.id), ActorId::User(bob));
        let from = Place { channel, classification: Classification::CHANNEL_DEFAULT, session: SessionPrivacy { id: session_id, ..SessionPrivacy::default() } };
        let views = manager.views(Viewer::from_place(context, from));
        let usable: Vec<(String, Scope)> = views.usable_skills().await.unwrap().iter().map(|s| (s.name().to_string(), s.scope().clone())).collect();
        assert_eq!(usable, [
            ("deploy".to_string(), Scope::UserScope { user_id: bob }),
            ("notes".to_string(), Scope::SessionScope { session_id }),
            ("review".to_string(), Scope::GlobalScope),
        ], "its owner's, the session's before the global one of a name, one shared with him; not one above the channel, not another's, not another session's");
        assert_eq!(views.usable_skill_named("Notes").await.unwrap().body().as_str(), "notes: how it is done", "found as names are compared");
        for absent in ["secret", "hers", "elsewhere", "nothing", ""] {
            assert!(matches!(views.usable_skill_named(absent).await, Err(ViewError::NoSkillNamed(_))), "{absent:?} answers as absent");
        }
        assert!(manager.views(as_user(bob)).usable_skills().await.unwrap().is_empty(), "a user loads none: skills are loaded into turns");
    }

    #[tokio::test]
    async fn an_agents_private_notes_are_read_from_every_channel_of_the_session_by_who_may() {
        use caelumora_model::ledger::Change;
        use caelumora_model::message::MessageId;
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let mut channel_of = |name: &str| {
            let channel = like(&template, name);
            session.channels.push(channel.id);
            channel
        };
        let (zeta, quiet, alpha) = (channel_of("zeta"), channel_of("quiet"), channel_of("alpha"));
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();
        for channel in [&zeta, &quiet, &alpha] {
            store.create_channel(&system, channel).await.unwrap();
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

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
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
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let channel = main_channel(&session);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

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

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
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
        let (store, manager) = owned_runtime_under(ConfidentialityRules::new(compartments));
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice]).await;
        session.participants.push(Participant::Agent(helper.id));
        let template = main_channel(&session);
        session.channels.clear();
        let system = SecurityContext::system();
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
        store.create_session(&system, &session).await.unwrap();
        for channel in [&public, &team, &secret, &hr] {
            store.create_channel(&system, channel).await.unwrap();
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

        let (store, manager) = owned_runtime();
        let alice = user("alice");
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice]).await;
        // The agent took part, said something, and was removed.
        let channel = main_channel(&session);
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
        deleted.name = Name::new("deleted").unwrap();
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
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&store, &[&alice, &bob]).await;
        let channel = main_channel(&session);
        let side = like(&channel, "side");
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
        let mut session = make_session("hers", ActorId::User(alice.id));
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
