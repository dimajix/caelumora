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
//! and answers what is left; the holder refuses its deletion if anything is
//! (`ScopeNotEmpty`): it is kept, and what is left is plain to see until
//! its owners take it elsewhere or delete it. Restoring restores what the
//! caller may and leaves the rest deleted, for its owners to restore: the
//! holder comes back all the same. Purging, the system's or an
//! administrator's, is done in the runtime's name.

use async_trait::async_trait;
use caelumora_model::scope::{Scope, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResult, Stored};
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

/// What is left in the scope of something being deleted, which the caller
/// may not delete (issue 320): what they may was deleted, the holder is
/// kept. `left` says how many of each kind are left - not which, since they
/// need not be the caller's to read. The message is meant for users, and it
/// answers for itself wherever it travels - alone or in the error of what
/// holds the scope (`error::user_message`, the REST API's `409
/// scope_not_empty`).
#[derive(Debug, thiserror::Error)]
#[error("The {} is not deleted: {} of what lies in its scope may not be deleted by you", shown_holder(.holder), shown_left(.left))]
pub struct ScopeNotEmpty {
    /// The scope of what was to be deleted: a session's or a channel's.
    pub holder: Scope,
    /// How many of each kind are left.
    pub left: Vec<(EntityKind, usize)>,
}

impl ScopeNotEmpty {
    /// Refuses the deletion of what holds `holder` if anything is `left`.
    pub(crate) fn require_empty(holder: &Scope, left: Vec<(EntityKind, usize)>) -> Result<(), Self> {
        if left.is_empty() {
            return Ok(());
        }
        Err(Self { holder: holder.clone(), left })
    }
}

/// What holds the scope `holder`, as an error names it.
fn shown_holder(holder: &Scope) -> String {
    match holder {
        Scope::SessionScope { session_id } => format!("session {session_id}"),
        Scope::ChannelScope { session_id, channel_id } => format!("channel {channel_id} of session {session_id}"),
        // Nothing else holds a scope that is deleted with it.
        _ => "holder of the scope".to_string(),
    }
}

/// How many of each kind are left, as an error says it: "1 agent, 2 workspace".
fn shown_left(left: &[(EntityKind, usize)]) -> String {
    left.iter().map(|(kind, count)| format!("{count} {kind}")).collect::<Vec<_>>().join(", ")
}

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
    /// deletion. Answers what is left, by kind - empty if nothing is -,
    /// which the holder refuses its deletion for (`ScopeNotEmpty`).
    ///
    /// Called once the holder is marked deleted, so that what goes is
    /// deleted with it, not before it, and comes back with it
    /// (`restore_lying_in`); if something is left, the caller takes the
    /// mark away again.
    ///
    /// # Errors
    /// Whatever the store reports; what was deleted before stays deleted.
    pub(crate) async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<Vec<(EntityKind, usize)>> {
        let mut left = Vec::new();
        for member in &self.members {
            let count = member.delete_lying_in(ctx, holder).await?;
            if count > 0 {
                left.push((member.kind(), count));
            }
        }
        Ok(left)
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
pub(crate) struct Unowned<T: Stored>(pub(crate) Arc<dyn EntityStore<T>>);

#[async_trait]
impl<T> ScopeMember for Unowned<T>
where
    T: Stored,
    T::Field: ScopedField,
{
    fn kind(&self) -> EntityKind {
        T::KIND
    }

    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize> {
        let mut left = 0;
        for entity in self.0.list(&SecurityContext::system(), &Query::filtered(Filter::lying_within(holder))).await?.items {
            left += usize::from(!deleted_unless_refused(self.0.delete(ctx, entity.id()).await)?);
        }
        Ok(left)
    }

    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        for entity in deleted_since(self.0.list_deleted(&SecurityContext::system(), &Query::filtered(Filter::lying_within(holder))).await?.items, since) {
            restored_unless_refused(self.0.restore(ctx, entity.id()).await, &entity)?;
        }
        Ok(())
    }

    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        for entity in self.0.list_deleted(&system, &Query::filtered(Filter::lying_within(holder))).await?.items {
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
        Err(e) if e.is_absent_or_refused() => {
            info!(kind = %D::KIND, id = %deleted.id(), "Stays deleted: it is not the caller's to restore");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Those of `deleted` that were deleted at `since` or later - a comparison
/// a query does not make yet.
pub fn deleted_since<D: Stored>(deleted: Vec<D>, since: DateTime<Utc>) -> Vec<D> {
    deleted.into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
