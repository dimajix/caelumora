//! `WorkspaceManager`: making, deleting, restoring and purging workspaces (issues 251, 253) -
//! stored first, then announced -, each with its place in its storage, and reaching them for the
//! runtime's other operations.

use super::{require_classification, store_error, WorkspaceError, WorkspaceResult};
use crate::backend::NamedBackends;
use crate::event::EventBus;
use crate::scoped::{purge_for_good, ScopeHolders, ScopeMember};
use crate::terminal::TerminalProvider;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::config::WorkspacesConfig;
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use caelumora_model::store::{
    purge_behind, EntityStore, EntityStores, StoreError, StoreResult, Stored,
};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

pub struct WorkspaceManager {
    workspaces: Arc<dyn EntityStore<Workspace>>,
    /// Asked whether a workspace may be put into a scope.
    holders: Arc<ScopeHolders>,
    events: Arc<EventBus>,
    access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where, and who is cleared for what (issue 304).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// The storages, by name, in which it makes and removes places; shared
    /// with the `ControllerManager`, which gives a workspace's controller
    /// its own, and the `CommandReaper`.
    storages: Arc<NamedBackends<dyn TerminalProvider>>,
    max_file_bytes: u64,
}

impl fmt::Debug for WorkspaceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceManager").field("storages", &self.storages).finish_non_exhaustive()
    }
}

impl WorkspaceManager {
    /// The manager of the workspaces of `stores`, whose files lie in
    /// `storages`, by their names - the storages its configuration names,
    /// made as backends (issue 324) -, and of which a person may put files
    /// as large as `config` says.
    pub(crate) fn new(
        stores: &EntityStores,
        holders: Arc<ScopeHolders>,
        events: Arc<EventBus>,
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        storages: Arc<NamedBackends<dyn TerminalProvider>>,
        config: &WorkspacesConfig,
    ) -> Self {
        Self {
            workspaces: Arc::clone(&stores.workspaces),
            holders,
            events,
            access_policy,
            confidentiality,
            storages,
            max_file_bytes: config.max_file_bytes,
        }
    }

    /// The largest file a person may put into a workspace.
    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    /// The configured storages, by name, each with whether its workspaces
    /// have a shell - the kind of workspace it makes (issue 266).
    pub fn storages(&self) -> impl Iterator<Item = (&str, bool)> {
        self.storages.iter().map(|(name, provider)| (name, provider.has_shell()))
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
    /// `NotFound` if there is no such session, channel or project they may
    /// read, `NameTaken`, and `Validation` for a scope a workspace does not
    /// lie in - a channel's or a project's they may read - or a
    /// `TerminalError` of making the place.
    pub async fn create(
        &self,
        ctx: &SecurityContext,
        name: &str,
        scope: Scope,
        storage: &str,
        description: Option<&str>,
        classification: Classification,
    ) -> WorkspaceResult<Workspace> {
        let ActorId::User(owner) = ctx.principal_id else {
            return Err(WorkspaceError::OwnerNotAUser);
        };
        let provider = self
            .storages
            .get(storage)
            .ok_or_else(|| WorkspaceError::UnknownStorage(storage.to_string()))?;
        let mut workspace =
            Workspace::new(ScopedName::new(name, scope)?, storage, classification, owner);
        workspace.description = Description::optional(description)?;
        require_classification(
            self.confidentiality.as_ref(),
            ctx,
            storage,
            provider.as_ref(),
            &workspace.classification,
        )?;
        if let Scope::SessionScope { .. } = workspace.name.scope {
            self.access_policy
                .require(
                    ctx,
                    ActionType::create(Workspace::KIND, &workspace.security_attributes),
                    StoreError::PermissionDenied,
                )
                .await?;
        }
        self.holders.require(ctx, &workspace.name.scope).await?;

        self.workspaces.create(ctx, &mut workspace).await?;
        if let Err(e) = provider.create(workspace.id).await {
            warn!(workspace = %workspace.id, "The place of a new workspace could not be made: {e}");
            if let Err(cleanup) = purge_for_good(self.workspaces.as_ref(), workspace.id).await {
                warn!(workspace = %workspace.id, "A workspace whose place could not be made is left deleted: {cleanup}");
            }
            return Err(e.into());
        }
        self.holders.keep_made(self, &workspace).await?;
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
        let workspace = match self.reached(ctx, id).await {
            Ok(workspace) => workspace,
            Err(WorkspaceError::NotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        if !self.workspaces.delete(ctx, id, &Removal::now(ctx.caller_id)).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<Workspace>(&id), &workspace);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace deleted");
        Ok(true)
    }

    /// Takes the deletion of workspace `id` back (`Restore`), with its
    /// files, if what holds its scope is still there (issue 458). Returns
    /// the workspace as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted, not the caller's to restore or its
    /// purge is begun (issue 492), or if what holds its scope is gone, or
    /// whatever else the store reports - `NameTaken` among it.
    pub async fn restore(
        &self,
        ctx: &SecurityContext,
        id: WorkspaceId,
    ) -> WorkspaceResult<Workspace> {
        let workspace = self.holders.restore(self, ctx, id).await.map_err(store_error(id))?;
        self.events.announce(GlobalEvent::restored::<Workspace>(&id), &workspace);
        info!(workspace = %id, by = %ctx.caller_id, "Workspace restored");
        Ok(workspace)
    }

    /// Removes the deleted workspace `id` for good, its place first, in the order of every purge of
    /// what has something behind it (`purge_behind`); `Purge` is asked before anything is removed.
    /// A place that cannot be removed leaves the workspace deleted, its purge begun, so that the
    /// next purge tries again. Announces nothing, as no purge does. `false` if there was nothing
    /// deleted the caller could see.
    ///
    /// # Errors
    /// `PermissionDenied`, `StorageGone` if its storage is no longer configured, a `TerminalError`
    /// of removing the place, or whatever the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<bool> {
        let purged = purge_behind(self.workspaces.as_ref(), ctx, id, async |workspace| {
            self.remove_place(workspace).await
        })
        .await?;
        if purged {
            info!(workspace = %id, by = %ctx.caller_id, "Workspace purged");
        }
        Ok(purged)
    }

    /// Removes the place of `workspace`, which is about to be purged.
    ///
    /// # Errors
    /// `StorageGone` if its storage is no longer configured, or a `TerminalError` of removing it.
    async fn remove_place(&self, workspace: &Workspace) -> WorkspaceResult<()> {
        self.storages.kept_in(&workspace.storage)?.remove(workspace.id).await?;
        Ok(())
    }

    /// The workspace of `name`, in exactly that scope, among those `ctx`
    /// may read and is cleared for - as a template looks for the one it
    /// names.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is none; whatever else the store
    /// reports.
    pub(crate) async fn find_by_name(
        &self,
        ctx: &SecurityContext,
        name: &ScopedName,
    ) -> StoreResult<Workspace> {
        self.workspaces.find_by_name(ctx, name).await
    }

    /// Workspace `id` as `ctx` reaches it, as the views reach it: by the
    /// store - its access list lets them read it (`GetWorkspace`) and they
    /// are cleared for its classification (issues 304, 322).
    ///
    /// # Errors
    /// `NotFound` if there is none they may read or are cleared for, so
    /// that nothing can be probed; or whatever else the store reports.
    async fn reached(&self, ctx: &SecurityContext, id: WorkspaceId) -> WorkspaceResult<Workspace> {
        self.workspaces.get(ctx, id).await.map_err(store_error(id))
    }

    /// Workspace `id` as `ctx` reaches it to attach it to a session (issue 253): reached as
    /// [`Self::reached`] reaches it, and theirs to attach (`UpdateWorkspace` - its owner, issue
    /// 251, decision 12).
    ///
    /// # Errors
    /// As [`Self::reached`]; `StoreError::PermissionDenied` if they may not change it.
    pub(crate) async fn reached_to_attach(
        &self,
        ctx: &SecurityContext,
        id: WorkspaceId,
    ) -> WorkspaceResult<Workspace> {
        let workspace = self.reached(ctx, id).await?;
        self.access_policy
            .require(
                ctx,
                ActionType::update(Workspace::KIND, &workspace.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        Ok(workspace)
    }
}

/// Workspaces go with what holds the scope they lie in, their places kept while they are deleted,
/// announced as `delete` and `restore` announce them, and purged with their places (`scoped`).
#[async_trait]
impl ScopeMember for WorkspaceManager {
    type Entity = Workspace;

    fn store(&self) -> &dyn EntityStore<Workspace> {
        self.workspaces.as_ref()
    }

    fn events(&self) -> Option<&dyn EventPublisher> {
        Some(self.events.as_ref())
    }

    /// Removes the place of `workspace`, as `purge` does; it is its storage too.
    ///
    /// # Errors
    /// As `StoreError::Backend`, what `remove_place` reports.
    async fn remove_behind(&self, workspace: &Workspace) -> StoreResult<()> {
        self.remove_place(workspace).await.map_err(|e| {
            StoreError::Backend(format!(
                "the place of workspace {} cannot be removed: {e}",
                workspace.id
            ))
        })
    }
}
