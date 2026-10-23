//! What lies in a scope goes with what holds it (issue 178): the
//! personalities, session templates, component bundles, agents, workspaces,
//! skills and artifacts of a session's scope - and of the scopes of its channels - are
//! deleted with the session, restored with it and purged with it, a
//! workspace with its place (issue 251), an artifact with its content
//! (issue 271); those of a channel's scope are deleted and restored with the
//! channel. `SessionManager` and `SessionLoop` are the callers, through
//! `ScopedEntities`. The group of the participants of a session or channel
//! lies in its scope too, but is not among it: it belongs to the session or
//! channel itself, which deletes and restores it with its own mark (issue
//! 348, `GroupManager::delete_participants`).
//!
//! Each kind that may lie in a scope is a `ScopeMember` (issue 323), kept
//! by `ScopedEntities` in the order it was registered. What has an owner of
//! its own - agents, workspaces, skills and artifacts - is a member
//! by its owner (`…_lying_in`, issue 319): it announces what it deletes and
//! restores as it announces its own deletions, and holds its lock of
//! writing as it does. Personalities, session templates and component
//! bundles, which have none, are members by their store (`Unowned`), which
//! marks them alone.
//!
//! What lies in the scope is listed in the runtime's name, so that all of
//! it is found, whatever the caller may read; nothing of the list reaches
//! the caller. It is deleted and restored in the caller's name (issue 320):
//! the store asks each as it asks of anything deleted or restored alone,
//! and marks it as deleted by the caller. What may lie in a session's scope
//! is not always its owner's - an artifact another participant published
//! there, a contributor's agent -, so deleting deletes what the caller may
//! and fails with `SessionError::ScopeNotEmpty` if anything is left: the
//! holder is kept, and what is left is plain to see until its owners take
//! it elsewhere or delete it. Restoring restores what the caller may and
//! leaves the rest deleted, for its owners to restore: the holder comes
//! back all the same. Purging, the system's or an administrator's, is done
//! in the runtime's name.

use crate::session::SessionError;
use async_trait::async_trait;
use caelumora_model::removal::Deletable;
use caelumora_model::scope::{Scope, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::query::Query;
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResult, Stored};
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};
use anyhow::Result;

/// A kind that may lie in a scope and goes with what holds it (issue 178):
/// deleted, restored and purged with the session or channel whose scope it
/// lies in. Implemented by the owner of the kind, which announces what it
/// deletes and restores, or by `Unowned` for a kind without one; a
/// plugin's kind is registered as one (`RuntimeBuilder::scope_member`).
#[async_trait]
pub trait ScopeMember: Send + Sync {
    /// The kind this member deletes, restores and purges.
    fn kind(&self) -> EntityKind;

    /// Deletes what of its kind lies in `holder` or within it on behalf of
    /// `ctx`, as far as they may; answers how many are left.
    ///
    /// # Errors
    /// Whatever else the store reports; what was deleted before stays
    /// deleted.
    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize>;

    /// Restores what of its kind lies in `holder` or within it and was
    /// deleted at `since` or later on behalf of `ctx`. One whose name was
    /// taken meanwhile stays deleted, as does one `ctx` may not restore.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()>;

    /// Purges what of its kind lies in `holder` or within it and is
    /// deleted, with what its owner purges with it, in the runtime's name.
    ///
    /// # Errors
    /// Whatever the store reports, and as `StoreError::Backend` a failure
    /// to remove what lies behind an entity - a workspace's place, an
    /// artifact's content -, which is its storage too; what was purged
    /// before stays purged.
    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()>;
}

/// Deletes, restores and purges what lies in a scope with what holds it
/// (issue 178), each kind by its `ScopeMember`, in the order they were
/// registered.
pub(crate) struct ScopedEntities {
    members: Vec<Arc<dyn ScopeMember>>,
}

impl fmt::Debug for ScopedEntities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedEntities").finish_non_exhaustive()
    }
}

impl ScopedEntities {
    /// Goes through `members` in their order.
    pub(crate) fn new(members: Vec<Arc<dyn ScopeMember>>) -> Self {
        Self { members }
    }

    /// Deletes what lies in `holder` or within it on behalf of `ctx`, as
    /// far as they may delete it, and announces it as its owner announces a
    /// deletion.
    ///
    /// Called once the holder is marked deleted, so that what goes is
    /// deleted with it, not before it, and comes back with it
    /// (`restore_lying_in`); if something is left, the caller takes the
    /// mark away again.
    ///
    /// # Errors
    /// `SessionError::ScopeNotEmpty` if something is left that `ctx` may
    /// not delete - what they may is deleted all the same -, or whatever
    /// the store reports; what was deleted before stays deleted.
    pub(crate) async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> Result<()> {
        let mut left = Vec::new();
        for member in &self.members {
            let count = member.delete_lying_in(ctx, holder).await?;
            if count > 0 {
                left.push((member.kind(), count));
            }
        }
        if !left.is_empty() {
            return Err(SessionError::ScopeNotEmpty { holder: holder.clone(), left }.into());
        }
        Ok(())
    }

    /// Restores what lies in `holder` or within it and was deleted with it -
    /// at `since`, when `holder` was deleted, or later - on behalf of `ctx`,
    /// and announces it as its owner announces a restoring. What was
    /// deleted before stays deleted, as a channel deleted before its
    /// session does; so does one whose name was taken meanwhile (issue 250)
    /// and one `ctx` may not restore (issue 320), and the others are
    /// restored all the same.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    pub(crate) async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        for member in &self.members {
            member.restore_lying_in(ctx, holder, since).await?;
        }
        Ok(())
    }

    /// Purges what lies in `holder` or within it and is deleted, with what
    /// its owner purges with it: the agents' state, the workspaces' places,
    /// the artifacts' content.
    ///
    /// # Errors
    /// Whatever the store reports, a place or a content that cannot be
    /// removed among it (`ScopeMember::purge_lying_in`); what was purged
    /// before stays purged.
    pub(crate) async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        for member in &self.members {
            member.purge_lying_in(holder).await?;
        }
        Ok(())
    }
}

/// A kind without an owner of its own - personalities and session
/// templates - as a `ScopeMember`: marked and purged in its store alone,
/// with nothing announced.
pub(crate) struct Unowned<T: Deletable>(pub(crate) Arc<dyn EntityStore<T>>);

#[async_trait]
impl<T> ScopeMember for Unowned<T>
where
    T: Deletable,
    T::Field: ScopedField,
{
    fn kind(&self) -> EntityKind {
        T::KIND
    }

    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize> {
        let mut left = 0;
        for entity in self.0.list(&SecurityContext::system(), &within(holder)).await?.items {
            left += usize::from(!deleted_unless_refused(self.0.delete(ctx, entity.id()).await)?);
        }
        Ok(left)
    }

    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        for entity in deleted_since(self.0.list_deleted(&SecurityContext::system(), &within(holder)).await?.items, since) {
            restored_unless_refused(self.0.restore(ctx, entity.id()).await, &entity)?;
        }
        Ok(())
    }

    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        for entity in self.0.list_deleted(&system, &within(holder)).await?.items {
            self.0.purge(&system, entity.id()).await?;
        }
        Ok(())
    }
}

/// Whether `deleted` says it was deleted; `false` also if it was not the
/// caller's to delete (issue 320), which leaves it.
pub fn deleted_unless_refused(deleted: StoreResult<bool>) -> StoreResult<bool> {
    match deleted {
        Err(StoreError::PermissionDenied(_)) => Ok(false),
        deleted => deleted,
    }
}

/// What `restored` says was restored of `deleted`; `None` if it stays
/// deleted: its name was taken meanwhile, or it is not the caller's to
/// restore (issue 320).
pub fn restored_unless_refused<D: Stored, T>(restored: StoreResult<T>, deleted: &D) -> StoreResult<Option<T>> {
    match restored {
        Ok(value) => Ok(Some(value)),
        Err(StoreError::NameTaken { .. }) => {
            warn!(kind = %D::KIND, id = %deleted.id(), "Stays deleted: its name was taken while what holds its scope was deleted");
            Ok(None)
        }
        Err(StoreError::NotFound { .. } | StoreError::PermissionDenied(_)) => {
            info!(kind = %D::KIND, id = %deleted.id(), "Stays deleted: it is not the caller's to restore");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The query of what lies in `holder` or within it (issue 293), which the
/// store answers by the scope.
pub fn within<F: ScopedField>(holder: &Scope) -> Query<F> {
    Query { filter: holder.within(), ..Query::all() }
}

/// Those of `deleted` that were deleted at `since` or later - a comparison
/// a query does not make yet.
pub fn deleted_since<D: Stored>(deleted: Vec<D>, since: DateTime<Utc>) -> Vec<D> {
    deleted.into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
