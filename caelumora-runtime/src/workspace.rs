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
//!
//! A command in a workspace is its channel's - the scope of its shell -
//! and runs on only as long as some current member of the channel could
//! start it there, as `shell_in` asks (issue 254): a task ends every other
//! command its storage lists, at each event that can change the answer and
//! besides every `COMMANDS_CHECKED_EVERY`.

use crate::component::{ComponentError, ComponentResult};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::terminal::{CommandKey, CommandState, DockerProvider, LocalProvider, Shell, SharedProvider, Terminal, TerminalError, TerminalProvider};
use caelumora_model::actor::ActorId;
use caelumora_model::config::{StorageConfig, WorkspacesConfig};
use caelumora_model::confidentiality::Classification;
use caelumora_model::event::GlobalEvent;
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions, Place};
use caelumora_model::security::{AccessQuestions, AclEntry, ActionType, Enforcement, Grant, SecurityContext, SecurityManager, Subject};
use crate::view::{channel_reading, ChannelReader};
use caelumora_model::channel::Participant;
use caelumora_model::session::{Channel, ChannelId, Session, SessionId};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResultExt};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_model::workspace::{Workspace, WorkspaceId, WorkspaceUpdateEvent};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, Mutex};
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, warn};

/// How often the commands of the storages are checked besides the events
/// that can change who may run them (issue 254).
const COMMANDS_CHECKED_EVERY: Duration = Duration::from_secs(60);

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
    #[error("you may read the workspace {0}, not change its files")]
    ReadOnly(WorkspaceId),
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
    /// What may flow where, and who is cleared for what (issue 304).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// Where the contexts of a channel's members are built, with their
    /// roles as they are now (issue 304).
    controllers: Arc<ControllerManager>,
    storages: BTreeMap<String, Arc<dyn TerminalProvider>>,
    max_file_bytes: u64,
    /// Held by every write of a document, so that none is lost to another.
    writing: Mutex<()>,
    /// The task that ends the commands nobody may run any longer; ends with
    /// the manager.
    checker: OnceLock<AbortOnDropHandle<()>>,
}

impl fmt::Debug for WorkspaceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceManager").field("storages", &self.storages.keys().collect::<Vec<_>>()).finish_non_exhaustive()
    }
}

impl WorkspaceManager {
    /// The manager of the storages `config` names. Where one of them has a
    /// shell, it starts the task that ends the commands nobody may run any
    /// longer (issue 254).
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a storage whose provider cannot
    /// be made, naming it, or for a storage with a shell outside a Tokio
    /// runtime.
    pub(crate) fn new(entities: Arc<dyn EntityStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>, confidentiality: Arc<dyn ConfidentialityPolicy>, controllers: Arc<ControllerManager>, secrets: &Arc<SecretManager>, config: &WorkspacesConfig) -> ComponentResult<Arc<Self>> {
        let storages: BTreeMap<String, Arc<dyn TerminalProvider>> = config.storage.iter()
            .map(|(name, storage)| {
                let invalid = |e: TerminalError| ComponentError::InvalidConfig(format!("the storage '{name}' of workspaces: {e}"));
                let max_run = storage.max_run();
                let provider: Arc<dyn TerminalProvider> = match storage {
                    StorageConfig::Local { root, shell, destination, .. } => Arc::new(LocalProvider::new(root.clone(), *shell, max_run, destination.clone())),
                    StorageConfig::OpenTerminalShared { endpoint, mount, destination, .. } => Arc::new(
                        SharedProvider::new(endpoint.clone(), mount, max_run, destination.clone(), Arc::clone(secrets)).map_err(invalid)?,
                    ),
                    StorageConfig::OpenTerminalDocker { image, network, idle_stop_secs, destination, .. } => Arc::new(
                        DockerProvider::new(name, image, network, Duration::from_secs(*idle_stop_secs), max_run, destination.clone()).map_err(invalid)?,
                    ),
                };
                Ok((name.clone(), provider))
            })
            .collect::<ComponentResult<_>>()?;
        let with_shell = storages.values().any(|provider| provider.has_shell());
        let manager = Arc::new(Self { entities, events, security, confidentiality, controllers, storages, max_file_bytes: config.max_file_bytes, writing: Mutex::new(()), checker: OnceLock::new() });
        if with_shell {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|_| ComponentError::InvalidConfig("a storage of workspaces with a shell needs a running runtime".to_string()))?;
            let subscription = manager.events.subscribe_all().into_receiver();
            let _ = manager.checker.set(AbortOnDropHandle::new(runtime.spawn(check_commands(Arc::downgrade(&manager), subscription))));
        }
        Ok(manager)
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
    /// putting it there (issue 251) - all of it the `ConfidentialityPolicy`'s
    /// (issue 304).
    async fn require_classification(&self, ctx: &SecurityContext, storage: &str, provider: &dyn TerminalProvider, classification: &Classification) -> WorkspaceResult<()> {
        let policy = &self.confidentiality;
        policy.require_classifiable(classification, || WorkspaceError::ClassificationRefused)?;
        if let Some(destination) = provider.destination() {
            policy.require_publish(ctx, classification, destination, || WorkspaceError::StorageNotCleared(storage.to_string()))?;
        }
        policy.require_cleared_for(ctx, classification, || WorkspaceError::NotCleared)
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

    /// Gives workspace `id` the access list `acl` (issue 23) on behalf of
    /// `ctx`: its owner's act (`UpdateWorkspace`), asked before a group the
    /// list names newly is looked at - who names one must read it (issue
    /// 286) - and again as it is stored. Announced without the list, which
    /// only its owner sees.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, a `StoreError` -
    /// `PermissionDenied` for anyone but its owner, `Validation` for an
    /// entry that names an agent, the system or nobody, or a group the
    /// caller cannot read.
    pub async fn set_access(&self, ctx: &SecurityContext, id: WorkspaceId, acl: Vec<AclEntry>) -> WorkspaceResult<Workspace> {
        let _writing = self.writing.lock().await;
        let mut workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        self.security.require(ctx, ActionType::UpdateWorkspace(&workspace.security_attributes), || StoreError::PermissionDenied).await?;
        crate::group::require_nameable(self.entities.as_ref(), ctx, &workspace.security_attributes.acl, &acl).await?;
        workspace.security_attributes.acl = acl;
        workspace.updated_at = chrono::Utc::now();
        self.entities.update_workspace(ctx, &workspace).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::WorkspaceUpdated(id, WorkspaceUpdateEvent::AccessChanged), workspace.security_attributes.clone());
        info!(workspace = %id, by = %ctx.caller_id, "Workspace's access changed");
        Ok(workspace)
    }

    /// Shares workspace `id` with `subject` as far as `grant` reaches, on
    /// behalf of `ctx`: an entry added to its access list, unless one
    /// already grants `subject` as much - one that grants less is widened.
    ///
    /// # Errors
    /// As [`Self::set_access`].
    pub async fn share(&self, ctx: &SecurityContext, id: WorkspaceId, subject: Subject, grant: Grant) -> WorkspaceResult<Workspace> {
        let workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        let mut acl = workspace.security_attributes.acl.clone();
        if acl.iter().any(|entry| entry.subject == subject && entry.grant.covers(grant)) {
            return Ok(workspace);
        }
        acl.retain(|entry| entry.subject != subject);
        acl.push(AclEntry { subject, grant });
        self.set_access(ctx, id, acl).await
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
            // Changed by `set_access`, which announces it without the list.
            WorkspaceUpdateEvent::AccessChanged => {}
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

    /// Workspace `id`, used directly by whoever `ctx` acts for, wherever the
    /// caller acts (issue 23): reached as the views reach it (issue 304) - its
    /// access list lets them read it (`GetWorkspace`) and they are cleared
    /// for its classification -, then if it grants them what `need` asks
    /// for (`WriteWorkspace`).
    ///
    /// # Errors
    /// `NotFound` if there is none they may read or are cleared for, so
    /// that nothing can be probed; `ReadOnly` if they may not change its
    /// files where `need` asks to; or whatever else the store reports.
    pub(crate) async fn reached(&self, ctx: &SecurityContext, id: WorkspaceId, need: Grant) -> WorkspaceResult<Workspace> {
        let workspace = self.entities.get_workspace(ctx, id).await.map_err(store_error(id))?;
        if !self.confidentiality.cleared_for(ctx, &workspace.classification) {
            return Err(WorkspaceError::NotFound(id));
        }
        if need == Grant::Write {
            self.security.require(ctx, ActionType::WriteWorkspace(&workspace.security_attributes), || WorkspaceError::ReadOnly(id)).await?;
        }
        Ok(workspace)
    }

    /// The workspace of `session`, if the caller of `ctx` may use it in
    /// `channel` (issue 251, decision 10): the channel is the session's and
    /// uses the workspace, the caller is one of its current members, its
    /// access list lets `ctx` read it (`GetWorkspace`, issue 23) and the
    /// workspace and the channel flow both ways - asked as the policy asks
    /// what an actor puts into a channel (`ConfidentialityPolicy::may_publish`). `None` otherwise, also
    /// when the workspace is gone, without saying which of them failed.
    /// Whether its files may be changed there is asked as they are
    /// (`terminal_in`).
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
        if !self.security.may_read_workspace(ctx, &workspace.security_attributes).await {
            return Ok(None);
        }
        let (inside, outside) = (&workspace.classification, &channel.classification);
        let flows = self.confidentiality.may_publish(ctx, inside, outside) && self.confidentiality.may_publish(ctx, outside, inside);
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
        for workspace in self.entities.list_deleted_workspaces(&system, &crate::scoped::within(holder)).await?.items {
            self.provider(&workspace)?.remove(workspace.id).await?;
            self.entities.purge_workspace(&system, workspace.id).await?;
        }
        Ok(())
    }

    /// Whether the terminals of `workspace` have a shell - asked of its
    /// storage, without reaching one; `false` for a storage no longer
    /// configured.
    pub(crate) fn has_shell(&self, workspace: &Workspace) -> bool {
        self.provider(workspace).is_ok_and(|provider| provider.has_shell())
    }

    /// Workspace `id` and its terminal, used directly by whoever `ctx` acts
    /// for, for what `need` asks - reading its files, or changing them too
    /// (issue 23) -, wherever they act: as `reached` reaches it.
    ///
    /// # Errors
    /// What `reached` reports, `UnknownStorage`, or a `TerminalError` of
    /// reaching its place.
    pub async fn terminal(&self, ctx: &SecurityContext, id: WorkspaceId, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let workspace = self.reached(ctx, id, need).await?;
        let terminal = self.place(&workspace).await?;
        Ok((workspace, terminal))
    }

    /// The workspace of session `session_id` and its terminal, for what
    /// `need` asks, if whoever `ctx` acts for may use it in its channel
    /// `channel_id` (issue 296): if they may read the session (`GetSession`)
    /// and the channel (`view::channel_reading` - an agent from
    /// the channel it acts in, a user by their roles), it is usable there
    /// (`usable_in`), and - to change its files - its list lets them
    /// (`WriteWorkspace`, issue 23). Session and channel are read as they
    /// are stored now, in the runtime's name, and asked of afresh: an
    /// operation in a channel relies on nothing its caller reached before.
    ///
    /// # Errors
    /// `NotUsableInChannel` if they may not use one there, `ReadOnly` if
    /// they may not change its files where `need` asks to, `UnknownStorage`,
    /// a `TerminalError` of reaching its place, or what the store reports.
    pub async fn terminal_in(&self, ctx: &SecurityContext, session_id: SessionId, channel_id: ChannelId, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let workspace = self.usable_in_channel(ctx, session_id, channel_id, need).await?;
        let terminal = self.place(&workspace).await?;
        Ok((workspace, terminal))
    }

    /// The workspace of session `session_id` and its shell as channel
    /// `channel_id` sees it - the commands started in that channel alone
    /// (issues 251, 296) -, if whoever `ctx` acts for may change its files
    /// there, as `terminal_in` asks; no shell where its storage gives none.
    /// The one place that makes a channel the scope of a shell.
    ///
    /// # Errors
    /// What `terminal_in` reports.
    pub async fn shell_in(&self, ctx: &SecurityContext, session_id: SessionId, channel_id: ChannelId) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let (workspace, terminal) = self.terminal_in(ctx, session_id, channel_id, Grant::Write).await?;
        let shell = terminal.shell(&scope_of(channel_id));
        Ok((workspace, shell))
    }

    /// Ends the command `id` of the workspace's shell that was started in
    /// channel `channel_id` of session `session_id`, with everything it
    /// started (issue 294), in the name of whoever `ctx` acts for: who may
    /// change the files there - who could have started it - may end it, as
    /// `shell_in` asks. Answers what it printed since `offset`, and how it
    /// stands.
    ///
    /// # Errors
    /// What `shell_in` reports, or `Terminal(UnknownCommand)` if no command
    /// `id` is known in the channel - as where its storage gives no shell.
    pub async fn kill_command(&self, ctx: &SecurityContext, session_id: SessionId, channel_id: ChannelId, id: &str, offset: u64) -> WorkspaceResult<CommandState> {
        let (_, shell) = self.shell_in(ctx, session_id, channel_id).await?;
        let shell = shell.ok_or_else(|| TerminalError::UnknownCommand(id.to_string()))?;
        Ok(shell.kill(id, offset).await?)
    }

    /// The workspace of session `session_id`, if whoever `ctx` acts for may
    /// use it in its channel `channel_id` for what `need` asks, as
    /// `terminal_in` asks.
    async fn usable_in_channel(&self, ctx: &SecurityContext, session_id: SessionId, channel_id: ChannelId, need: Grant) -> WorkspaceResult<Workspace> {
        let system = SecurityContext::system();
        let Some(session) = self.entities.get_session(&system, session_id).await.optional()? else { return Err(WorkspaceError::NotUsableInChannel) };
        let Some(channel) = self.entities.get_channel(&system, channel_id).await.optional()? else { return Err(WorkspaceError::NotUsableInChannel) };
        if !self.security.may_read_session(ctx, &session.security_attributes).await {
            return Err(WorkspaceError::NotUsableInChannel);
        }
        let at = Place { channel: channel.id, classification: channel.classification.clone(), session: SessionPrivacy::of(&session) };
        let reader = match ctx.caller_id {
            ActorId::Agent(_) => ChannelReader::From(&at),
            _ => ChannelReader::Cleared,
        };
        if channel_reading(self.security.as_ref(), self.confidentiality.as_ref(), ctx, &channel.security_attributes, &at, reader).await.is_none() {
            return Err(WorkspaceError::NotUsableInChannel);
        }
        let workspace = self.usable_in(ctx, &session, &channel).await?.ok_or(WorkspaceError::NotUsableInChannel)?;
        if need == Grant::Write {
            self.security.require(ctx, ActionType::WriteWorkspace(&workspace.security_attributes), || WorkspaceError::ReadOnly(workspace.id)).await?;
        }
        Ok(workspace)
    }

    /// Ends every command of the storages that nobody may run any longer
    /// where it was started (issue 254). One whose question cannot be
    /// answered now runs on until it can.
    async fn end_unusable_commands(&self) {
        for provider in self.storages.values() {
            let mut ending = Vec::new();
            for key in provider.commands() {
                match self.may_run(&key).await {
                    Ok(true) => {}
                    Ok(false) => ending.push(key),
                    Err(e) => warn!(workspace = %key.0, command = %key.2, "Whether a command may still run could not be asked: {e}"),
                }
            }
            end_commands(provider.as_ref(), ending, "nobody may run it any longer where it was started").await;
        }
    }

    /// Ends every command started in one of `channels`, with what it
    /// started, without asking anything: what stops a session ends what
    /// runs in its channels (issue 254).
    pub(crate) async fn end_commands_in(&self, channels: &[ChannelId]) {
        let scopes: Vec<String> = channels.iter().copied().map(scope_of).collect();
        for provider in self.storages.values() {
            let ending = provider.commands().into_iter().filter(|(_, scope, _)| scopes.contains(scope)).collect();
            end_commands(provider.as_ref(), ending, "its session stopped").await;
        }
    }

    /// Whether some current member of the channel command `key` was started
    /// in - its scope - could start it there, as `shell_in` asks: a command
    /// is its channel's, not its starter's (issue 254). Asked in each
    /// member's name, with the runtime as initiator: it asks on its own.
    ///
    /// # Errors
    /// What the store reports.
    async fn may_run(&self, (workspace, scope, _): &CommandKey) -> WorkspaceResult<bool> {
        let system = SecurityContext::system();
        let Some(channel_id) = channel_of(scope) else { return Ok(false) };
        let Some(channel) = self.entities.get_channel(&system, channel_id).await.optional()? else { return Ok(false) };
        for participant in &channel.participants {
            let member = match participant {
                // One whose record is gone asks with no roles, as before.
                Participant::User(user) => match self.controllers.user_controller(*user).await.optional()? {
                    Some(controller) => controller.security_context(channel.session_id, channel.id),
                    None => SecurityContext::new(ActorId::User(*user), channel.session_id, channel.id),
                },
                Participant::Agent(agent) => match self.controllers.agent_controller(*agent).await.optional()? {
                    Some(agent) => {
                        let limit = agent.personality().await.optional()?.and_then(|p| p.limiting_roles().map(<[_]>::to_vec));
                        agent.security_context(channel.session_id, channel.id, limit.as_deref())
                    }
                    None => continue,
                },
                Participant::Approver(_) => continue,
            };
            let ctx = SecurityContext { initiator_id: ActorId::System, ..member };
            match self.usable_in_channel(&ctx, channel.session_id, channel.id, Grant::Write).await {
                Ok(usable) if usable.id == *workspace => return Ok(true),
                Ok(_) | Err(WorkspaceError::NotUsableInChannel | WorkspaceError::ReadOnly(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    /// The terminal of `workspace`, which one of the ways of using it let
    /// the caller reach.
    ///
    /// # Errors
    /// `UnknownStorage` if its storage is no longer configured, or a
    /// `TerminalError` of reaching its place.
    async fn place(&self, workspace: &Workspace) -> WorkspaceResult<Arc<dyn Terminal>> {
        Ok(self.provider(workspace)?.terminal_for(workspace.id).await?)
    }
}

/// The scope of the shell a workspace has in channel `id`: a command is its
/// channel's (issues 251, 296).
fn scope_of(id: ChannelId) -> String {
    id.to_string()
}

/// The channel scope `scope` is; `None` for one that is no channel's.
fn channel_of(scope: &str) -> Option<ChannelId> {
    scope.parse::<uuid::Uuid>().ok().map(ChannelId::from)
}

/// Ends the commands `keys` of `provider`, for the reason `why`; one that
/// cannot be ended runs on, logged.
async fn end_commands(provider: &dyn TerminalProvider, keys: Vec<CommandKey>, why: &str) {
    for key in keys {
        info!(workspace = %key.0, command = %key.2, "Ending a command: {why}");
        if let Err(e) = provider.end_command(&key).await {
            warn!(workspace = %key.0, command = %key.2, "A command to end could not be ended: {e}");
        }
    }
}

/// Ends, at every event that can change who may use a workspace in a
/// channel and besides every `COMMANDS_CHECKED_EVERY`, the commands nobody
/// may run any longer (issue 254), until `manager` or the bus is gone.
/// Events it fell behind on are a reason to check, as any is.
async fn check_commands(manager: Weak<WorkspaceManager>, mut events: broadcast::Receiver<GlobalEvent>) {
    let mut every = tokio::time::interval(COMMANDS_CHECKED_EVERY);
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) if !changes_who_may_run(&event) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            _ = every.tick() => {}
        }
        let Some(manager) = manager.upgrade() else { return };
        manager.end_unusable_commands().await;
    }
}

/// Whether `event` can change who may run a command in a channel: the
/// session, the channel, its members, the workspace or who someone is.
fn changes_who_may_run(event: &GlobalEvent) -> bool {
    matches!(event,
        GlobalEvent::SessionUpdated(..) | GlobalEvent::SessionDeleted(_)
        | GlobalEvent::ChannelUpdated(..) | GlobalEvent::ChannelDeleted(..)
        | GlobalEvent::AgentChannelLeft(..) | GlobalEvent::UserChannelLeft(..)
        | GlobalEvent::AgentUpdated(..) | GlobalEvent::AgentDeleted(_)
        | GlobalEvent::UserUpdated(..) | GlobalEvent::UserDeleted(_)
        | GlobalEvent::WorkspaceUpdated(..) | GlobalEvent::WorkspaceDeleted(_))
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
    use caelumora_model::config::{ArtifactStorageConfig, ArtifactsConfig, StorageConfig, WorkspacesConfig, DEFAULT_MAX_RUN_SECS};
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
        let storage = StorageConfig::Local { root: files.path().to_path_buf(), shell: true, destination: Some(internal.clone()), max_run_secs: DEFAULT_MAX_RUN_SECS };
        let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
        let artifacts = tempfile::tempdir().unwrap();
        let kept = ArtifactsConfig { storage: [("kept".to_string(), ArtifactStorageConfig::Local { root: artifacts.path().to_path_buf(), destination: None })].into(), ..Default::default() };
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()), &caelumora_model::config::AppConfig { workspaces, artifacts: kept, ..Default::default() }, crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
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
        WithWorkspace { files, artifacts, session, agent: agent.id, context, workspace, runtime }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{session_with_workspace, WithWorkspace};
    use super::*;
    use crate::terminal::CommandStatus;
    use caelumora_model::channel::ChannelUpdateEvent;

    /// A command started by the agent in the session's channel, still
    /// running, with its shell.
    async fn running(w: &WithWorkspace) -> (Arc<dyn Shell>, String) {
        let channel = w.session.channels()[0].id();
        let (_, shell) = w.runtime.workspace_manager().shell_in(&w.context, w.session.id(), channel).await.unwrap();
        let shell = shell.expect("its storage gives a shell");
        let id = shell.start("sleep 30").await.unwrap();
        (shell, id)
    }

    /// Whether the channel uses the workspace.
    async fn use_workspace(w: &WithWorkspace, uses: bool) -> Channel {
        let store = w.runtime.entity_store();
        let mut channel = store.get_channel(&SecurityContext::system(), w.session.channels()[0].id()).await.unwrap();
        channel.uses_workspace = uses;
        store.update_channel(&SecurityContext::system(), &channel).await.unwrap();
        channel
    }

    #[tokio::test]
    async fn a_command_runs_on_while_its_channel_may_use_the_workspace_and_ends_once_it_may_not() {
        let w = session_with_workspace().await;
        let workspaces = w.runtime.workspace_manager();
        let (shell, id) = running(&w).await;

        workspaces.end_unusable_commands().await;
        assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Running, "the channel may still use it");

        use_workspace(&w, false).await;
        workspaces.end_unusable_commands().await;
        assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Killed, "the channel no longer uses the workspace");
    }

    #[tokio::test]
    async fn a_command_runs_on_as_long_as_some_member_could_start_it_there() {
        let w = session_with_workspace().await;
        let workspaces = w.runtime.workspace_manager();
        let (shell, id) = running(&w).await;
        let store = w.runtime.entity_store();
        let mut channel = store.get_channel(&SecurityContext::system(), w.session.channels()[0].id()).await.unwrap();

        channel.participants.retain(|participant| !matches!(participant, Participant::Agent(_)));
        store.update_channel(&SecurityContext::system(), &channel).await.unwrap();
        workspaces.end_unusable_commands().await;
        assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Running, "the agent that started it left, the owner may still start it there");

        channel.participants.clear();
        store.update_channel(&SecurityContext::system(), &channel).await.unwrap();
        workspaces.end_unusable_commands().await;
        assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Killed, "nobody left who could");
    }

    #[tokio::test]
    async fn a_change_announced_ends_a_command_at_once() {
        let w = session_with_workspace().await;
        let (shell, id) = running(&w).await;
        let channel = use_workspace(&w, false).await;
        w.runtime.event_bus().publish(GlobalEvent::ChannelUpdated(channel.session_id, channel.id, ChannelUpdateEvent::UsesWorkspaceChanged(false)), channel.security_attributes.clone());
        let ended = shell.output(&id, 0, Duration::from_secs(10)).await.unwrap();
        assert_eq!(ended.status, CommandStatus::Killed, "not only at the next round");
    }
}
