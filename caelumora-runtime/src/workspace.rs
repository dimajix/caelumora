//! The one owner of "a workspace and its place" (issues 251, 253): the
//! writer of workspace documents - stored first, then announced - and the
//! only code that holds the `TerminalProvider`s, one per configured
//! storage. Reading the documents is the views' (`view::WorkspaceView`).
//!
//! A workspace is used in one of two ways, each checked here before
//! anything else (issue 251, decisions 10 to 12):
//!
//! - *directly*, by its owner, outside a session (`owned`): `GetWorkspace`,
//!   and the owner cleared for its classification;
//! - *in a channel* of the session it is attached to (`usable_in`): by a
//!   current member of the channel, if the channel uses the workspace and
//!   the workspace and the channel flow both ways - what a command prints
//!   reaches the channel, and what was seen there may go into a command.
//!   No `GetWorkspace`: attaching shared it with the session's channels.

use crate::event::EventBus;
use crate::terminal::{LocalProvider, Terminal, TerminalError, TerminalProvider};
use caelumora_model::actor::ActorId;
use caelumora_model::config::{StorageConfig, WorkspacesConfig};
use caelumora_model::confidentiality::{user_roles, Classification};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::session::{Channel, Session};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResultExt};
use caelumora_model::value::name::Name;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_model::workspace::{Workspace, WorkspaceId, WorkspaceUpdateEvent};
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
    pub(crate) fn new(entities: Arc<dyn EntityStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>, config: &WorkspacesConfig) -> Self {
        let storages = config.storage.iter()
            .map(|(name, storage)| {
                let provider: Arc<dyn TerminalProvider> = match storage {
                    StorageConfig::Local { root } => Arc::new(LocalProvider::new(root.clone())),
                };
                (name.clone(), provider)
            })
            .collect();
        Self { entities, events, security, storages, max_file_bytes: config.max_file_bytes, writing: Mutex::new(()) }
    }

    /// The largest file a person may put into a workspace.
    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    /// The configured storages, by name.
    pub fn storages(&self) -> impl Iterator<Item = &str> {
        self.storages.keys().map(String::as_str)
    }

    fn provider(&self, workspace: &Workspace) -> WorkspaceResult<&Arc<dyn TerminalProvider>> {
        self.storages.get(&workspace.storage).ok_or_else(|| WorkspaceError::UnknownStorage(workspace.storage.clone()))
    }

    /// Whether whoever `ctx` acts for may give `classification`, and is
    /// cleared for it - as they would have to be to read a channel so
    /// classified.
    async fn require_classification(&self, ctx: &SecurityContext, classification: &Classification) -> WorkspaceResult<()> {
        self.security.require(ctx, ActionType::Classify(classification), || WorkspaceError::ClassificationRefused).await?;
        let roles = user_roles(self.entities.as_ref(), ctx.initiator_id).await?;
        match self.security.may_see_channel(ctx, &roles, classification).await {
            true => Ok(()),
            false => Err(WorkspaceError::NotCleared),
        }
    }

    /// Makes a workspace of the user `ctx` acts for, in `storage`: the
    /// document, then its place there. A place that cannot be made leaves
    /// no document.
    ///
    /// # Errors
    /// `OwnerNotAUser`, `UnknownStorage`, `Invalid` for a name or
    /// description that does not fit, `ClassificationRefused`, `NotCleared`,
    /// a `StoreError` - `NameTaken` among them - or a `TerminalError` of
    /// making the place.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, storage: &str, description: Option<&str>, classification: Classification) -> WorkspaceResult<Workspace> {
        let ActorId::User(owner) = ctx.initiator_id else { return Err(WorkspaceError::OwnerNotAUser) };
        let provider = self.storages.get(storage).ok_or_else(|| WorkspaceError::UnknownStorage(storage.to_string()))?;
        let mut workspace = Workspace::new(Name::new(name)?, storage, classification, owner);
        workspace.description = Description::optional(description)?;
        self.require_classification(ctx, &workspace.classification).await?;

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
            WorkspaceUpdateEvent::NameChanged(name) => workspace.name = Name::new(name)?,
            WorkspaceUpdateEvent::DescriptionChanged(description) => workspace.description = Description::optional(description.as_deref())?,
            WorkspaceUpdateEvent::ClassificationChanged(classification) => {
                self.require_classification(ctx, classification).await?;
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
        let roles = user_roles(self.entities.as_ref(), ctx.initiator_id).await?;
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
        let (inside, outside) = (&workspace.classification, &channel.classification);
        let flows = self.security.may_publish(ctx, ctx.caller_id, inside, outside).await
            && self.security.may_publish(ctx, ctx.caller_id, outside, inside).await;
        Ok(flows.then_some(workspace))
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
