//! The one owner of "a workspace and its place" (issues 251, 253): it
//! makes, changes, deletes and purges workspaces - stored first, then
//! announced - and is the only code that holds the `TerminalProvider`s,
//! one per configured storage. Reading the documents is the views'
//! (`view::WorkspaceView`); deleting and restoring them with what holds
//! their scope is `scoped.rs`'s, which marks them as it marks every kind.
//!
//! A workspace is used in one of two ways, each checked here before
//! anything else (issue 251, decisions 10 to 12):
//!
//! - *directly*, by its owner, outside a session (`owned`): `GetWorkspace`,
//!   and the owner cleared for its classification - not one that lies in
//!   a session's scope, which is used within its session alone;
//! - *in a channel* of the session it is attached to (`usable_in`): by a
//!   current member of the channel, if the channel uses the workspace and
//!   the workspace and the channel flow both ways - what a command prints
//!   reaches the channel, and what was seen there may go into a command.
//!   No `GetWorkspace`: attaching shared it with the session's channels.
//!
//! A workspace lies in a scope as an agent does (issue 251, decision 13):
//! what lies in a session's scope is used within that session only
//! (`Scope::admits`), and is deleted, restored and purged with it - its
//! place removed by `purge_lying_in`.

use crate::component::{ComponentError, ComponentResult};
use crate::event::EventBus;
use crate::terminal::{DockerProvider, LocalProvider, SharedProvider, Terminal, TerminalError, TerminalProvider};
use caelumora_model::actor::ActorId;
use caelumora_model::config::{StorageConfig, WorkspacesConfig};
use caelumora_model::confidentiality::{user_roles, Classification};
use caelumora_model::event::GlobalEvent;
use caelumora_model::scope::{Scope, Scoped};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::session::{Channel, Session};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResultExt};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_model::workspace::{Workspace, WorkspaceId, WorkspaceUpdateEvent};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// No such workspace, or not one the caller may use - one case, so
    /// that nothing can be probed.
    #[error("there is no workspace {0}")]
    NotFound(WorkspaceId),
    #[error("no storage named '{0}' is configured")]
    UnknownStorage(String),
    #[error("a workspace belongs to a user")]
    OwnerNotAUser,
    #[error("the classification may not be given")]
    ClassificationRefused,
    #[error("you are not cleared for the classification of the workspace")]
    NotCleared,
    /// Its storage may not hold what is so classified (issue 251): a place
    /// with a shell or a remote side is cleared for so much.
    #[error("the storage '{0}' may not hold a workspace so classified")]
    StorageNotCleared(String),
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

/// `error` of the store about workspace `id`: not found is one case with
/// not readable.
fn store_error(id: WorkspaceId) -> impl FnOnce(StoreError) -> WorkspaceError {
    move |error| match error {
        StoreError::NotFound { .. } => WorkspaceError::NotFound(id),
        error => WorkspaceError::Store(error),
    }
}

pub struct WorkspaceManager {
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
    security: Arc<dyn SecurityManager>,
    storages: BTreeMap<String, Arc<dyn TerminalProvider>>,
    max_file_bytes: u64,
    /// Held by every write of a document, so that none is lost to another.
    writing: Mutex<()>,
}

impl fmt::Debug for WorkspaceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceManager").field("storages", &self.storages.keys().collect::<Vec<_>>()).finish_non_exhaustive()
    }
}

impl WorkspaceManager {
    /// The manager of the storages `config` names.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a storage whose provider cannot
    /// be made, naming it.
    pub(crate) fn new(entities: Arc<dyn EntityStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>, secrets: &Arc<SecretManager>, config: &WorkspacesConfig) -> ComponentResult<Self> {
        let storages = config.storage.iter()
            .map(|(name, storage)| {
                let invalid = |e: TerminalError| ComponentError::InvalidConfig(format!("the storage '{name}' of workspaces: {e}"));
                let provider: Arc<dyn TerminalProvider> = match storage {
                    StorageConfig::Local { root, shell, destination } => Arc::new(LocalProvider::new(root.clone(), *shell, destination.clone())),
                    StorageConfig::OpenTerminalShared { endpoint, mount, destination } => Arc::new(
                        SharedProvider::new(endpoint.clone(), mount, destination.clone(), Arc::clone(secrets)).map_err(invalid)?,
                    ),
                    StorageConfig::OpenTerminalDocker { image, network, idle_stop_secs, destination } => Arc::new(
                        DockerProvider::new(name, image, network, Duration::from_secs(*idle_stop_secs), destination.clone()).map_err(invalid)?,
                    ),
                };
                Ok((name.clone(), provider))
            })
            .collect::<ComponentResult<_>>()?;
        Ok(Self { entities, events, security, storages, max_file_bytes: config.max_file_bytes, writing: Mutex::new(()) })
    }

    /// The largest file a person may put into a workspace.
    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    /// The configured storages, by name, each with whether its workspaces
    /// have a shell - the kind of workspace it makes (issue 266).
    pub fn storages(&self) -> impl Iterator<Item = (&str, bool)> {
        self.storages.iter().map(|(name, provider)| (name.as_str(), provider.has_shell()))
    }

    fn provider(&self, workspace: &Workspace) -> WorkspaceResult<&Arc<dyn TerminalProvider>> {
        self.storages.get(&workspace.storage).ok_or_else(|| WorkspaceError::UnknownStorage(workspace.storage.clone()))
    }

    /// Whether whoever `ctx` acts for may give `classification` to a
    /// workspace in `storage`, and is cleared for it - as they would have
    /// to be to read a channel so classified -, and whether the storage may
    /// hold it: what is so classified flows into its destination, asked as
    /// putting it there (`Publish`, issue 251).
    async fn require_classification(&self, ctx: &SecurityContext, storage: &str, provider: &dyn TerminalProvider, classification: &Classification) -> WorkspaceResult<()> {
        self.security.require(ctx, ActionType::Classify(classification), || WorkspaceError::ClassificationRefused).await?;
        if let Some(destination) = provider.destination() {
            if !self.security.may_publish(ctx, ctx.caller_id, classification, destination).await {
                return Err(WorkspaceError::StorageNotCleared(storage.to_string()));
            }
        }
        let roles = user_roles(self.entities.as_ref(), ctx.principal_id).await?;
        match self.security.may_see_channel(ctx, &roles, classification).await {
            true => Ok(()),
            false => Err(WorkspaceError::NotCleared),
        }
    }

    /// Makes a workspace of the user `ctx` acts for, of `name` in `scope`,
    /// in `storage`: the document, then its place there. A place that
    /// cannot be made leaves no document.
    ///
    /// # Errors
    /// `OwnerNotAUser`, `UnknownStorage`, `Invalid` for a name or
    /// description that does not fit, `ClassificationRefused`, `NotCleared`,
    /// a `StoreError` - `NameTaken` among them, and `Validation` for a
    /// scope a workspace does not lie in - or a `TerminalError` of making
    /// the place.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, scope: Scope, storage: &str, description: Option<&str>, classification: Classification) -> WorkspaceResult<Workspace> {
        let ActorId::User(owner) = ctx.principal_id else { return Err(WorkspaceError::OwnerNotAUser) };
        let provider = self.storages.get(storage).ok_or_else(|| WorkspaceError::UnknownStorage(storage.to_string()))?;
        let mut workspace = Workspace::new(ScopedName::new(name, scope)?, storage, classification, owner);
        workspace.description = Description::optional(description)?;
        self.require_classification(ctx, storage, provider.as_ref(), &workspace.classification).await?;

        let _writing = self.writing.lock().await;
        self.entities.create_workspace(ctx, &workspace).await?;
        if let Err(e) = provider.create(workspace.id).await {
            warn!(workspace = %workspace.id, "The place of a new workspace could not be made: {e}");
            let system = SecurityContext::system();
            if let Err(cleanup) = async {
                self.entities.delete_workspace(&system, workspace.id).await?;
                self.entities.purge_workspace(&system, workspace.id).await
            }.await {
                warn!(workspace = %workspace.id, "A workspace whose place could not be made is left in the trash: {cleanup}");
            }
            return Err(e.into());
        }
        self.events.publish(GlobalEvent::WorkspaceCreated(workspace.id), workspace.security_attributes.clone());
        info!(workspace = %workspace.id, storage, by = %ctx.caller_id, "Workspace created");
        Ok(workspace)
    }

    /// Renames workspace `id` on behalf of `ctx` (`UpdateWorkspace`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rename(&self, ctx: &SecurityContext, id: WorkspaceId, name: &str) -> WorkspaceResult<Workspace> {
        self.update(ctx, id, WorkspaceUpdateEvent::NameChanged(name.to_string())).await
    }

    /// Says what workspace `id` holds; `None` or empty takes it away.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn describe(&self, ctx: &SecurityContext, id: WorkspaceId, description: Option<&str>) -> WorkspaceResult<Workspace> {
        self.update(ctx, id, WorkspaceUpdateEvent::DescriptionChanged(description.map(String::from))).await
    }

    /// Reclassifies workspace `id` as a whole: lowering releases every file
    /// in it. Its owner must be cleared for the new classification; which
    /// channels may use it follows at their next use.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn reclassify(&self, ctx: &SecurityContext, id: WorkspaceId, classification: Classification) -> WorkspaceResult<Workspace> {
        self.update(ctx, id, WorkspaceUpdateEvent::ClassificationChanged(classification)).await
    }

    /// Applies `event` to workspace `id` on behalf of `ctx`: stored first,
    /// which is where the permission is checked (`UpdateWorkspace`), then
    /// announced.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, `Invalid` if what
    /// `event` says does not fit, `ClassificationRefused` or `NotCleared`
    /// for a classification, or whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, id: WorkspaceId, event: WorkspaceUpdateEvent) -> WorkspaceResult<Workspace> {
        let _writing = self.writing.lock().await;
        let mut workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        match &event {
            WorkspaceUpdateEvent::NameChanged(name) => workspace.name = ScopedName::new(name, workspace.name.scope.clone())?,
            WorkspaceUpdateEvent::DescriptionChanged(description) => workspace.description = Description::optional(description.as_deref())?,
            WorkspaceUpdateEvent::ClassificationChanged(classification) => {
                let provider = Arc::clone(self.provider(&workspace)?);
                self.require_classification(ctx, &workspace.storage, provider.as_ref(), classification).await?;
                workspace.classification = classification.clone();
            }
        }
        workspace.updated_at = chrono::Utc::now();
        self.entities.update_workspace(ctx, &workspace).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::WorkspaceUpdated(id, event), workspace.security_attributes.clone());
        info!(workspace = %id, by = %ctx.caller_id, "Workspace changed");
        Ok(workspace)
    }

    /// Deletes workspace `id` (`DeleteWorkspace`): marked, its files kept.
    /// A session it is attached to reads it as absent. `false` if there was
    /// none the caller could see.
    ///
    /// # Errors
    /// Whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(workspace) = self.entities.get_workspace(ctx, id).await.optional()? else { return Ok(false) };
        if !self.entities.delete_workspace(ctx, id).await? {
            return Ok(false);
        }
        self.events.publish(GlobalEvent::WorkspaceDeleted(id), workspace.security_attributes);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace deleted");
        Ok(true)
    }

    /// Takes the deletion of workspace `id` back (`Restore`), with its
    /// files.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore, or
    /// whatever else the store reports - `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<()> {
        let _writing = self.writing.lock().await;
        self.entities.restore_workspace(ctx, id).await.map_err(store_error(id))?;
        let workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::WorkspaceRestored(id), workspace.security_attributes);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace restored");
        Ok(())
    }

    /// Removes the deleted workspace `id` for good, its files first
    /// (`Purge`, asked before anything is removed). A place that cannot be
    /// removed leaves the document, so that the next purge tries again.
    /// Announces nothing, as no purge does. `false` if there was nothing
    /// deleted the caller could see.
    ///
    /// # Errors
    /// `PermissionDenied`, `UnknownStorage` if its storage is no longer
    /// configured, a `TerminalError` of removing the place, or whatever the
    /// store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(workspace) = self.entities.get_deleted_workspace(ctx, id).await.optional()? else { return Ok(false) };
        self.security.require(ctx, ActionType::Purge(EntityKind::Workspace, &workspace.security_attributes), || StoreError::PermissionDenied).await?;
        self.provider(&workspace)?.remove(id).await?;
        let purged = self.entities.purge_workspace(ctx, id).await?;
        info!(workspace = %id, by = %ctx.caller_id, "Workspace purged");
        Ok(purged)
    }

    /// Workspace `id`, used directly by whoever `ctx` acts for: its owner
    /// (`GetWorkspace`), cleared for its classification.
    ///
    /// # Errors
    /// `NotFound` if there is none they may use, `NotCleared`, or whatever
    /// else the store reports.
    pub(crate) async fn owned(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<Workspace> {
        let workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        // Outside every session: what lies in one's scope is used in it alone.
        if !workspace.name.scope.admits(&Scope::GlobalScope) {
            return Err(WorkspaceError::NotFound(id));
        }
        let roles = user_roles(self.entities.as_ref(), ctx.principal_id).await?;
        match self.security.may_see_channel(ctx, &roles, &workspace.classification).await {
            true => Ok(workspace),
            false => Err(WorkspaceError::NotCleared),
        }
    }

    /// The workspace of `session`, if the caller of `ctx` may use it in
    /// `channel` (issue 251, decision 10): the channel is the session's and
    /// uses the workspace, the caller is one of its current members, and
    /// the workspace and the channel flow both ways - asked as the policy
    /// asks what an actor puts into a channel (`Publish`). `None` otherwise,
    /// also when the workspace is gone, without saying which of them
    /// failed. The document is read in the runtime's name: a member need
    /// not be its owner.
    ///
    /// # Errors
    /// Whatever reading the workspace reports, other than its absence.
    pub(crate) async fn usable_in(&self, ctx: &SecurityContext, session: &Session, channel: &Channel) -> WorkspaceResult<Option<Workspace>> {
        let Some(id) = session.workspace else { return Ok(None) };
        let member = channel.participants.iter().any(|participant| participant.actor_id() == ctx.caller_id);
        if channel.session_id != session.id || !channel.uses_workspace || !member {
            return Ok(None);
        }
        let Some(workspace) = self.entities.get_workspace(&SecurityContext::system(), id).await.optional()? else {
            warn!(session = %session.id, workspace = %id, "The workspace attached to a session is gone");
            return Ok(None);
        };
        if !workspace.name.scope.admits(&Scope::SessionScope { session_id: session.id }) {
            warn!(session = %session.id, workspace = %id, "A workspace of another session's scope is attached to a session");
            return Ok(None);
        }
        let (inside, outside) = (&workspace.classification, &channel.classification);
        let flows = self.security.may_publish(ctx, ctx.caller_id, inside, outside).await
            && self.security.may_publish(ctx, ctx.caller_id, outside, inside).await;
        Ok(flows.then_some(workspace))
    }

    /// Purges the deleted workspaces that lie in `holder` or within it,
    /// their places first.
    ///
    /// # Errors
    /// `UnknownStorage`, a `TerminalError` of removing a place, or whatever
    /// the store reports; what was purged before stays purged.
    pub(crate) async fn purge_lying_in(&self, holder: &Scope) -> WorkspaceResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for workspace in self.entities.list_deleted_workspaces(&system).await? {
            if holder.contains(&workspace.scoped_name().scope) {
                self.provider(&workspace)?.remove(workspace.id).await?;
                self.entities.purge_workspace(&system, workspace.id).await?;
            }
        }
        Ok(())
    }

    /// Whether the terminals of `workspace` have a shell - asked of its
    /// storage, without reaching one; `false` for a storage no longer
    /// configured.
    pub(crate) fn has_shell(&self, workspace: &Workspace) -> bool {
        self.provider(workspace).is_ok_and(|provider| provider.has_shell())
    }

    /// The terminal of `workspace`, which one of the two ways of using it
    /// let the caller reach.
    ///
    /// # Errors
    /// `UnknownStorage` if its storage is no longer configured, or a
    /// `TerminalError` of reaching its place.
    pub(crate) async fn terminal(&self, workspace: &Workspace) -> WorkspaceResult<Arc<dyn Terminal>> {
        Ok(self.provider(workspace)?.terminal_for(workspace.id).await?)
    }
}

/// What the tests of the components that work on a workspace share: a
/// running session whose channel "main" an agent takes part in, with a
/// workspace attached that its storage gives a shell.
#[cfg(test)]
pub(crate) mod testing {
    use crate::agent::state::AgentStateHandle;
    use crate::agent::turn::Turn;
    use crate::runtime::RuntimeManager;
    use crate::session::SessionLoop;
    use crate::view::ViewsFactory;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::config::{ArtifactStorageConfig, ArtifactsConfig, StorageConfig, WorkspacesConfig};
    use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::EntityStore;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::workspace::Workspace;
    use std::sync::Arc;

    /// The largest file the workspace takes.
    pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;

    pub(crate) struct WithWorkspace {
        /// Where the storage keeps the workspace's files.
        pub(crate) files: tempfile::TempDir,
        /// Where the storage of artifacts keeps their content (issue 271).
        pub(crate) artifacts: tempfile::TempDir,
        pub(crate) session: Arc<SessionLoop>,
        pub(crate) agent: AgentId,
        /// What the agent acts in: for the workspace's owner.
        pub(crate) context: SecurityContext,
        pub(crate) workspace: Workspace,
    }

    impl WithWorkspace {
        /// A turn of the agent in the channel, begun in `state`.
        pub(crate) fn turn<'a>(&self, state: &'a mut AgentChannelState) -> Turn<'a> {
            let channel = &self.session.channels()[0];
            state.begin_turn(EventContext::new(self.session.id(), channel.id(), ActorId::None, String::new()), self.context.clone(), channel.classification(), Vec::new(), Default::default());
            Turn::of(self.agent, "Jonathan".to_string(), state, AgentStateHandle::detached()).expect("a turn was begun")
                .with_views(ViewsFactory::in_session(Arc::clone(&self.session)))
        }

        /// Where the workspace's files lie on the host.
        pub(crate) fn dir(&self) -> std::path::PathBuf {
            self.files.path().join(self.workspace.id.to_string())
        }
    }

    pub(crate) async fn session_with_workspace() -> WithWorkspace {
        let store = Arc::new(MemoryStore::new());
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let files = tempfile::tempdir().unwrap();
        let internal = Classification::general(Confidentiality::Internal);
        let storage = StorageConfig::Local { root: files.path().to_path_buf(), shell: true, destination: Some(internal.clone()) };
        let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
        let artifacts = tempfile::tempdir().unwrap();
        let kept = ArtifactsConfig { storage: [("kept".to_string(), ArtifactStorageConfig::Local { root: artifacts.path().to_path_buf(), destination: None })].into(), ..Default::default() };
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, &caelumora_model::config::AppConfig { workspaces, artifacts: kept, ..Default::default() }, crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let system = SecurityContext::system();

        let owner = UserId::new();
        let as_owner = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let workspace = runtime.workspace_manager().create(&as_owner, "docs", Scope::UserScope { user_id: owner }, "dev", Some("Drafts"), internal.clone()).await.unwrap();

        let personality = Personality {
            description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), &personality, ActorId::User(owner));
        store.create_agent(&system, &agent).await.unwrap();
        let session_id = SessionId::new();
        let channel = Channel::build(session_id, ActorId::User(owner), Name::new("main").unwrap(), Default::default(), internal, None, vec![Participant::User(owner), Participant::Agent(agent.id)]);
        let now = chrono::Utc::now();
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: Some(workspace.id),
            id: session_id, name: Name::new("s").unwrap(), participants: channel.participants.clone(), channels: vec![channel.id],
            commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
            deleted: None,
        };
        store.create_session(&system, &session).await.unwrap();
        store.create_channel(&system, &channel).await.unwrap();
        let session = SessionLoop::new(session, vec![channel], Arc::clone(&runtime)).await.unwrap();
        let context = SecurityContext::new(ActorId::User(owner), session_id, session.channels()[0].id()).next(ActorId::Agent(agent.id), ActorId::User(owner));
        WithWorkspace { files, artifacts, session, agent: agent.id, context, workspace }
    }
}
