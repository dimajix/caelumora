//! `WorkspaceManager`: making, deleting, restoring and purging workspaces
//! (issues 251, 253) - stored first, then announced -, each with its place
//! in its storage.

use super::{require_classification, store_error, WorkspaceError, WorkspaceResult};
use crate::event::EventBus;
use crate::scoped::{ScopeMember, deleted_since, deleted_unless_refused, restored_unless_refused};
use crate::terminal::TerminalProvider;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::config::WorkspacesConfig;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, AccessPolicy};
use caelumora_model::session::Session;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{EntityKind, EntityStore, EntityStores, StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub struct WorkspaceManager {
    workspaces: Arc<dyn EntityStore<Workspace>>,
    /// The sessions a workspace may be made in the scope of.
    sessions: Arc<dyn EntityStore<Session>>,
    events: Arc<EventBus>,
    access_policy: Arc<dyn AccessPolicy>,
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
    pub(crate) fn new(stores: &EntityStores, events: Arc<EventBus>, access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>, storages: Arc<BTreeMap<String, Arc<dyn TerminalProvider>>>, config: &WorkspacesConfig) -> Self {
        Self { workspaces: Arc::clone(&stores.workspaces), sessions: Arc::clone(&stores.sessions), events, access_policy, confidentiality, storages, max_file_bytes: config.max_file_bytes, writing: Mutex::new(()) }
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
            self.access_policy.require(ctx, ActionType::create(Workspace::KIND, &workspace.security_attributes), StoreError::PermissionDenied).await?;
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
        self.access_policy.require(ctx, ActionType::purge(Workspace::KIND, &workspace.security_attributes), StoreError::PermissionDenied).await?;
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
        for workspace in self.workspaces.list(&system, &Query::filtered(Filter::lying_within(holder))).await?.items {
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
        for workspace in deleted_since(self.workspaces.list_deleted(&system, &Query::filtered(Filter::lying_within(holder))).await?.items, since) {
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
        for workspace in self.workspaces.list_deleted(&system, &Query::filtered(Filter::lying_within(holder))).await?.items {
            // Its place is its storage too.
            let unremoved = |e: &dyn fmt::Display| StoreError::Backend(format!("the place of workspace {} cannot be removed: {e}", workspace.id));
            self.provider(&workspace).map_err(|e| unremoved(&e))?.remove(workspace.id).await.map_err(|e| unremoved(&e))?;
            self.workspaces.purge(&system, workspace.id).await?;
        }
        Ok(())
    }
}
