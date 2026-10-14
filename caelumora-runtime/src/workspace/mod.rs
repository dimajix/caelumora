//! The one owner of the lifecycle of "a workspace and its place" (issues
//! 251, 253): it makes, deletes, restores and purges workspaces - stored
//! first, then announced -, each with its place in its storage. What a
//! workspace is - its name, description, classification and access list -
//! its controller changes (`controller::WorkspaceController`, issue 344).
//! Reading the documents is the views' (`view::WorkspaceView`).
//!
//! A workspace is used in one of two ways, each asked by its controller
//! before anything else (issue 251, decisions 10 to 12):
//!
//! - *directly*, wherever the caller acts (`WorkspaceController::terminal`):
//!   `GetWorkspace`, and the caller cleared for its classification (issue
//!   23);
//! - *in a channel* of the session it is attached to
//!   (`WorkspaceController::terminal_in`): by a current member of the
//!   channel, if the channel uses the workspace and the workspace and the
//!   channel flow both ways - what a command prints reaches the channel,
//!   and what was seen there may go into a command.
//!
//! A workspace lies in a scope as an agent does (issue 251, decision 13):
//! what lies in a session's scope is used within that session only
//! (`Scope::admits`), and is deleted, restored and purged with it
//! (`…_lying_in`, called by `scoped`) - its place removed as it is purged.
//!
//! The commands run in workspaces are ended by the `CommandReaper`
//! (`reaper`) once nobody may run them any longer (issue 254).

pub(crate) mod reaper;

use crate::event::EventBus;
use crate::scoped::{ScopeMember, deleted_since, deleted_unless_refused, restored_unless_refused, within};
use crate::terminal::{TerminalError, TerminalProvider};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::config::WorkspacesConfig;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, Refused, SecurityContext, SecurityManager, WRITE};
use caelumora_model::session::Session;
use caelumora_model::store::{EntityKind, EntityStore, EntityStores, StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
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
    /// What its access list grants the caller is reading, not changing its
    /// files (issue 23).
    #[error("you may read the workspace {id}, not change its files")]
    ReadOnly { id: WorkspaceId, #[source] refused: Refused },
    /// No workspace the caller may use in the channel they act in: none is
    /// attached, the channel does not use it, they may not read the channel
    /// or the workspace, or the two do not flow both ways - one case, so
    /// that nothing can be probed (issue 296).
    #[error("there is no workspace you can use in this channel")]
    NotUsableInChannel,
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
pub(crate) fn store_error(id: WorkspaceId) -> impl FnOnce(StoreError) -> WorkspaceError {
    move |error| match error {
        StoreError::NotFound { .. } => WorkspaceError::NotFound(id),
        error => WorkspaceError::Store(error),
    }
}

/// Whether `classification` may be given at all, and whether the storage
/// `storage`, served by `provider`, may hold what is so classified - what
/// is so classified flows into its destination, asked as putting it there
/// (issue 251) - and whoever `ctx` acts for is cleared for it, as they
/// would have to be to read a channel so classified: the flow, the
/// `ConfidentialityPolicy`'s (issue 304).
///
/// # Errors
/// `ClassificationRefused`, `StorageNotCleared` or `NotCleared`.
pub(crate) fn require_classification(policy: &dyn ConfidentialityPolicy, ctx: &SecurityContext, storage: &str, provider: &dyn TerminalProvider, classification: &Classification) -> WorkspaceResult<()> {
    policy.require_classifiable(classification, || WorkspaceError::ClassificationRefused)?;
    if let Some(destination) = provider.destination() {
        policy.require_publish(ctx, classification, destination, || WorkspaceError::StorageNotCleared(storage.to_string()))?;
    }
    policy.require_cleared_for(ctx, classification, || WorkspaceError::NotCleared)
}

/// Whether whoever `ctx` acts for may change the files of `workspace`
/// (`WriteWorkspace`, issue 23), once they reached it.
///
/// # Errors
/// `ReadOnly` if they may not.
pub(crate) async fn require_write(security: &dyn SecurityManager, ctx: &SecurityContext, workspace: &Workspace) -> WorkspaceResult<()> {
    security.require(ctx, ActionType::custom(Workspace::KIND, &WRITE, &workspace.security_attributes), |refused| WorkspaceError::ReadOnly { id: workspace.id, refused }).await
}

pub struct WorkspaceManager {
    workspaces: Arc<dyn EntityStore<Workspace>>,
    /// The sessions a workspace may be made in the scope of.
    sessions: Arc<dyn EntityStore<Session>>,
    events: Arc<EventBus>,
    security: Arc<dyn SecurityManager>,
    /// What may flow where, and who is cleared for what (issue 304).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// The storages, by name, in which it makes and removes places; shared
    /// with the `ControllerManager`, which gives a workspace's controller
    /// its own, and the `CommandReaper`.
    storages: Arc<BTreeMap<String, Arc<dyn TerminalProvider>>>,
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
    /// The manager of the workspaces of `stores`, whose files lie in
    /// `storages`, by their names - the storages its configuration names,
    /// made as backends (issue 324) -, and of which a person may put files
    /// as large as `config` says.
    pub(crate) fn new(stores: &EntityStores, events: Arc<EventBus>, security: Arc<dyn SecurityManager>, confidentiality: Arc<dyn ConfidentialityPolicy>, storages: Arc<BTreeMap<String, Arc<dyn TerminalProvider>>>, config: &WorkspacesConfig) -> Self {
        Self { workspaces: Arc::clone(&stores.workspaces), sessions: Arc::clone(&stores.sessions), events, security, confidentiality, storages, max_file_bytes: config.max_file_bytes, writing: Mutex::new(()) }
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

    /// Makes a workspace of the user `ctx` acts for, of `name` in `scope`,
    /// in `storage`: the document, then its place there. A place that
    /// cannot be made leaves no document. What was asked for is validated,
    /// then its flow asked; creating it (`CreateWorkspace`) is the store's
    /// to ask, since nothing is changed and nothing of others asked before
    /// (issue 312).
    ///
    /// A workspace of a session's scope goes with the session: what lies
    /// there and cannot be deleted by who deletes the session keeps the
    /// session from being deleted (`ScopeNotEmpty`). So only who may read
    /// the session (`GetSession`) puts a workspace there, as an agent is put
    /// there (issue 344) - asked after making it was, which is then asked
    /// before anything of others.
    ///
    /// # Errors
    /// `OwnerNotAUser`, `UnknownStorage`, `Invalid` for a name or
    /// description that does not fit, `ClassificationRefused`, `NotCleared`,
    /// a `StoreError` - `PermissionDenied` if they may not make it,
    /// `NotFound` if there is no such session they may read, `NameTaken`,
    /// and `Validation` for a scope a workspace does not lie in - or a
    /// `TerminalError` of making the place.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, scope: Scope, storage: &str, description: Option<&str>, classification: Classification) -> WorkspaceResult<Workspace> {
        let ActorId::User(owner) = ctx.principal_id else { return Err(WorkspaceError::OwnerNotAUser) };
        let provider = self.storages.get(storage).ok_or_else(|| WorkspaceError::UnknownStorage(storage.to_string()))?;
        let mut workspace = Workspace::new(ScopedName::new(name, scope)?, storage, classification, owner);
        workspace.description = Description::optional(description)?;
        require_classification(self.confidentiality.as_ref(), ctx, storage, provider.as_ref(), &workspace.classification)?;
        if let Scope::SessionScope { session_id } = workspace.name.scope {
            self.security.require(ctx, ActionType::create(Workspace::KIND, &workspace.security_attributes), StoreError::PermissionDenied).await?;
            self.sessions.get(ctx, session_id).await?;
        }

        let _writing = self.writing.lock().await;
        self.workspaces.create(ctx, &workspace).await?;
        if let Err(e) = provider.create(workspace.id).await {
            warn!(workspace = %workspace.id, "The place of a new workspace could not be made: {e}");
            let system = SecurityContext::system();
            if let Err(cleanup) = async {
                self.workspaces.delete(&system, workspace.id).await?;
                self.workspaces.purge(&system, workspace.id).await
            }.await {
                warn!(workspace = %workspace.id, "A workspace whose place could not be made is left in the trash: {cleanup}");
            }
            return Err(e.into());
        }
        self.events.announce(GlobalEvent::created::<Workspace>(&workspace.id), &workspace);
        info!(workspace = %workspace.id, storage, by = %ctx.caller_id, "Workspace created");
        Ok(workspace)
    }

    /// Deletes workspace `id` (`DeleteWorkspace`): marked, its files kept.
    /// A session it is attached to reads it as absent. `false` if there was
    /// none the caller could reach, as the views reach it.
    ///
    /// # Errors
    /// Whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<bool> {
        let _writing = self.writing.lock().await;
        let workspace = match self.reached(ctx, id).await {
            Ok(workspace) => workspace,
            Err(WorkspaceError::NotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        if !self.workspaces.delete(ctx, id).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<Workspace>(&id), &workspace);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace deleted");
        Ok(true)
    }

    /// Takes the deletion of workspace `id` back (`Restore`), with its
    /// files. Returns the workspace as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore, or
    /// whatever else the store reports - `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<Workspace> {
        let _writing = self.writing.lock().await;
        let workspace = self.workspaces.restore(ctx, id).await.map_err(store_error(id))?;
        self.events.announce(GlobalEvent::restored::<Workspace>(&id), &workspace);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace restored");
        Ok(workspace)
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
        let Some(workspace) = self.workspaces.get_deleted(ctx, id).await.optional()? else { return Ok(false) };
        self.security.require(ctx, ActionType::purge(Workspace::KIND, &workspace.security_attributes), StoreError::PermissionDenied).await?;
        self.provider(&workspace)?.remove(id).await?;
        let purged = self.workspaces.purge(ctx, id).await?;
        info!(workspace = %id, by = %ctx.caller_id, "Workspace purged");
        Ok(purged)
    }

    /// The workspace of `name`, in exactly that scope, among those `ctx`
    /// may read and is cleared for - as a template looks for the one it
    /// names.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is none; whatever else the store
    /// reports.
    pub(crate) async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Workspace> {
        self.workspaces.find_by_name(ctx, name).await
    }

    /// Workspace `id` as `ctx` reaches it, as the views reach it: by the
    /// store - its access list lets them read it (`GetWorkspace`) and they
    /// are cleared for its classification (issues 304, 322).
    ///
    /// # Errors
    /// `NotFound` if there is none they may read or are cleared for, so
    /// that nothing can be probed; or whatever else the store reports.
    pub(crate) async fn reached(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<Workspace> {
        self.workspaces.get(ctx, id).await.map_err(store_error(id))
    }
}

#[async_trait]
impl ScopeMember for WorkspaceManager {
    fn kind(&self) -> EntityKind {
        Workspace::KIND
    }

    /// Deletes the workspaces that lie in `holder` or within it, their
    /// places kept, on behalf of `ctx` (`scoped`), as far as they may, and
    /// announces them as `delete` does; answers how many are left.
    ///
    /// # Errors
    /// Whatever else the store reports; what was deleted before stays
    /// deleted.
    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        let mut left = 0;
        for workspace in self.workspaces.list(&system, &within(holder)).await?.items {
            if deleted_unless_refused(self.workspaces.delete(ctx, workspace.id).await)? {
                self.events.announce(GlobalEvent::deleted::<Workspace>(&workspace.id), &workspace);
            } else {
                left += 1;
            }
        }
        Ok(left)
    }

    /// Restores the workspaces that lie in `holder` or within it and were
    /// deleted at `since` or later, with their places, on behalf of `ctx`
    /// (`scoped`), and announces them as `restore` does. One whose name was
    /// taken meanwhile stays deleted, as does one `ctx` may not restore.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for workspace in deleted_since(self.workspaces.list_deleted(&system, &within(holder)).await?.items, since) {
            if let Some(restored) = restored_unless_refused(self.workspaces.restore(ctx, workspace.id).await, &workspace)? {
                self.events.announce(GlobalEvent::restored::<Workspace>(&restored.id), &restored);
            }
        }
        Ok(())
    }

    /// Purges the deleted workspaces that lie in `holder` or within it,
    /// their places first.
    ///
    /// # Errors
    /// `UnknownStorage`, a `TerminalError` of removing a place, or whatever
    /// the store reports; what was purged before stays purged.
    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for workspace in self.workspaces.list_deleted(&system, &within(holder)).await?.items {
            // Its place is its storage too.
            let unremoved = |e: &dyn fmt::Display| StoreError::Backend(format!("the place of workspace {} cannot be removed: {e}", workspace.id));
            self.provider(&workspace).map_err(|e| unremoved(&e))?.remove(workspace.id).await.map_err(|e| unremoved(&e))?;
            self.workspaces.purge(&system, workspace.id).await?;
        }
        Ok(())
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
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, Participant};
    use caelumora_model::config::{BackendConfig, WorkspacesConfig};
    use serde_json::json;
    use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
    use caelumora_model::scope::Scope;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::workspace::Workspace;
    use std::sync::Arc;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::config::AppConfig;
    use crate::testing::test_registry;

    /// The largest file the workspace takes.
    pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;

    pub(crate) struct WithWorkspace {
        /// Where the storage keeps the workspace's files.
        pub(crate) files: tempfile::TempDir,
        pub(crate) session: Arc<SessionLoop>,
        pub(crate) agent: AgentId,
        /// What the agent acts in: for the workspace's owner.
        pub(crate) context: SecurityContext,
        pub(crate) workspace: Workspace,
        /// What it runs on: where skills are made (issue 22).
        pub(crate) runtime: Arc<RuntimeManager>,
    }

    impl WithWorkspace {
        /// A turn of the agent in the channel, begun in `state`.
        pub(crate) fn turn<'a>(&self, state: &'a mut AgentChannelState) -> Turn<'a> {
            let channel = &self.session.channels()[0];
            state.begin_turn(EventContext::new(self.session.id(), channel.id(), ActorId::None, String::new()), self.context.clone(), channel.classification(), Vec::new(), Default::default());
            Turn::of(self.agent, "Jonathan".to_string(), state, AgentStateHandle::detached(), crate::session::testing::views_in(&self.session, channel, ActorId::Agent(self.agent))).expect("a turn was begun")
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
        let storage = BackendConfig::new("local", json!({ "root": files.path(), "shell": true, "destination": { "level": "internal" } }));
        let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::new(ConfidentialityRules::default()), &AppConfig { workspaces, ..Default::default() }, test_registry(Arc::<MemoryStore>::clone(&store))).unwrap());
        let system = SecurityContext::system();

        let owner = UserId::new();
        let as_owner = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let workspace = runtime.workspace_manager().create(&as_owner, "docs", Scope::UserScope { user_id: owner }, "dev", Some("Drafts"), internal.clone()).await.unwrap();

        let personality = Personality {
            description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, deleted: None,
        };
        store.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), personality.id, ActorId::User(owner));
        store.entity_stores().agents.create(&system, &agent).await.unwrap();
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
        store.entity_stores().sessions.create(&system, &session).await.unwrap();
        store.entity_stores().channels.create(&system, &channel).await.unwrap();
        let session = SessionLoop::new(session, vec![channel], Arc::clone(&runtime)).await.unwrap();
        let context = SecurityContext::new(ActorId::User(owner), session_id, session.channels()[0].id()).next(ActorId::Agent(agent.id), ActorId::User(owner), ActorId::User(owner));
        WithWorkspace { files, session, agent: agent.id, context, workspace, runtime }
    }
}
