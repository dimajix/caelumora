//! Groups in the runtime (issues 286, 304): the groups of the participants
//! of sessions and channels, which those keep, and the teams users make.
//!
//! The participants of a session or a channel are a group an access list
//! names - the session's and the channel's own, a workspace attached to the
//! session, an artifact published in the channel. Its one writer is what
//! changes the participants: the `SessionManager` and `SessionLoop` as they
//! make a session or a channel, the `ChannelController` as someone joins or
//! leaves. The group is written in the runtime's name after the change of
//! the participants it follows, which is the act that was permitted; it is
//! derived anew from the participants each time, so that a write that
//! failed is corrected by the next. What lies in a session's or channel's
//! scope goes with it (`scoped`), the group of its participants among it:
//! the `GroupManager` deletes, restores and purges it with the session or
//! channel (`…_lying_in`).
//!
//! A team is made and changed by the `GroupManager`, its one writer, which
//! leaves the permissions to the store - who reads, changes and deletes a
//! group is the policy's (`GetGroup`, `UpdateGroup`, `DeleteGroup`); the
//! participants of a session or channel it does not change at all. Reading
//! groups is the views' (`view::GroupView`).
//!
//! Whoever names a group in an access list must be able to read it
//! (`require_nameable`): naming one grants its members something, and nobody
//! grants what they cannot tell to whom.

use crate::event::EventBus;
use crate::scoped::{deleted_since, unless_taken, within};
use caelumora_model::actor::ActorId;
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId, GroupUpdateEvent};
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, SecurityContext, Subject};
use caelumora_model::store::{EntityStore, StoreError, StoreResult, StoreResultExt};
use caelumora_model::user::UserId;
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::ValueError;
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    /// No such group, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no group {0}")]
    NotFound(GroupId),
    #[error("a team belongs to a user")]
    OwnerNotAUser,
    /// The participants of a session or a channel change by taking part in
    /// it, not by hand.
    #[error("group {0} is the participants of a session or channel and changes with them")]
    Participants(GroupId),
    #[error("there is no user {0}")]
    UnknownMember(UserId),
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type GroupResult<T> = Result<T, GroupError>;

/// `error` of the store about group `id`: not found is one case with not
/// readable.
fn store_error(id: GroupId) -> impl FnOnce(StoreError) -> GroupError {
    move |error| match error {
        StoreError::NotFound { .. } => GroupError::NotFound(id),
        error => GroupError::Store(error),
    }
}

/// The one writer of teams (issue 286).
pub struct GroupManager {
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
    /// Held by every write of a document, so that none is lost to another.
    writing: Mutex<()>,
}

impl fmt::Debug for GroupManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupManager").finish_non_exhaustive()
    }
}

impl GroupManager {
    pub(crate) fn new(entities: Arc<dyn EntityStore>, events: Arc<EventBus>) -> Self {
        Self { entities, events, writing: Mutex::new(()) }
    }

    /// Makes a team of the user `ctx` acts for, of `name` in `scope` - the
    /// global one or their own -, with nobody in it yet (`CreateGroup`).
    ///
    /// # Errors
    /// `OwnerNotAUser`, `Invalid` for a name that does not fit, or a
    /// `StoreError` - `NameTaken` among them, and `Validation` for a scope a
    /// team does not lie in.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, scope: Scope) -> GroupResult<Group> {
        let ActorId::User(owner) = ctx.principal_id else { return Err(GroupError::OwnerNotAUser) };
        let group = Group::team(ScopedName::new(name, scope)?, owner);
        let _writing = self.writing.lock().await;
        self.entities.create_group(ctx, &group).await?;
        self.events.publish(GlobalEvent::GroupCreated(group.id), group.security_attributes.clone());
        info!(group = %group.id, by = %ctx.caller_id, "Group created");
        Ok(group)
    }

    /// Renames team `id` on behalf of `ctx` (`UpdateGroup`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rename(&self, ctx: &SecurityContext, id: GroupId, name: &str) -> GroupResult<Group> {
        self.update(ctx, id, GroupUpdateEvent::NameChanged(name.to_string()), None).await
    }

    /// Makes `members` the members of team `id`, on behalf of `ctx`
    /// (`UpdateGroup`): who is no longer among them no longer reads what was
    /// shared with it.
    ///
    /// # Errors
    /// `UnknownMember` for a user that does not exist, otherwise as
    /// [`Self::update`].
    pub async fn set_members(&self, ctx: &SecurityContext, id: GroupId, members: Vec<UserId>) -> GroupResult<Group> {
        let system = SecurityContext::system();
        let mut actors = Vec::with_capacity(members.len());
        for user in members {
            if self.entities.get_user(&system, user).await.optional()?.is_none() {
                return Err(GroupError::UnknownMember(user));
            }
            if !actors.contains(&ActorId::User(user)) {
                actors.push(ActorId::User(user));
            }
        }
        self.update(ctx, id, GroupUpdateEvent::MembersChanged, Some(actors)).await
    }

    /// Applies `event` to team `id` on behalf of `ctx` - with `members` for
    /// its new members: stored first, which is where the permission is
    /// checked (`UpdateGroup`), then announced to whoever reads it, before
    /// and after.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, `Participants` for
    /// the participants of a session or channel, `Invalid` if a name does
    /// not fit, or whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, id: GroupId, event: GroupUpdateEvent, members: Option<Vec<ActorId>>) -> GroupResult<Group> {
        let _writing = self.writing.lock().await;
        let mut group = self.entities.get_group(ctx, id).await.map_err(store_error(id))?;
        if group.is_participants() {
            return Err(GroupError::Participants(id));
        }
        let before = group.security_attributes.clone();
        let mut told: Vec<ActorId> = group.members.clone();
        match &event {
            GroupUpdateEvent::NameChanged(name) => group.name = ScopedName::new(name, group.name.scope.clone())?,
            GroupUpdateEvent::MembersChanged => {}
        }
        if let Some(members) = members {
            group.members = members;
        }
        group.updated_at = Utc::now();
        self.entities.update_group(ctx, &group).await.map_err(store_error(id))?;
        // Who left is told too: they read it no longer once it is stored.
        told.retain(|actor| !group.has_member(*actor));
        let attributes = told.into_iter().fold(before, |attributes, actor| attributes.granting(Subject::Actor(actor), Grant::Read));
        self.events.publish(GlobalEvent::GroupUpdated(id, event), attributes);
        info!(group = %id, by = %ctx.caller_id, "Group changed");
        Ok(group)
    }

    /// Deletes team `id` (`DeleteGroup`): marked, kept; who it was shared
    /// with through it reads it no longer. `false` if there was none the
    /// caller could see.
    ///
    /// # Errors
    /// `Participants` for the participants of a session or channel, or
    /// whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(group) = self.entities.get_group(ctx, id).await.optional()? else { return Ok(false) };
        if group.is_participants() {
            return Err(GroupError::Participants(id));
        }
        if !self.entities.delete_group(ctx, id).await? {
            return Ok(false);
        }
        self.events.publish(GlobalEvent::GroupDeleted(id), group.security_attributes);
        info!(group = %id, by = %ctx.caller_id, "Group deleted");
        Ok(true)
    }

    /// Takes the deletion of team `id` back (`Restore`). Returns the team
    /// as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore,
    /// `Participants` for the participants of a session or channel, which
    /// come back with it, or whatever else the store reports - `NameTaken`
    /// among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<Group> {
        let _writing = self.writing.lock().await;
        let deleted = self.entities.get_deleted_group(ctx, id).await.map_err(store_error(id))?;
        if deleted.is_participants() {
            return Err(GroupError::Participants(id));
        }
        let group = self.entities.restore_group(ctx, id).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::GroupRestored(id), group.security_attributes.clone());
        info!(group = %id, by = %ctx.caller_id, "Group restored");
        Ok(group)
    }

    /// Removes the deleted team `id` for good (`Purge`, asked by the store).
    /// Announces nothing, as no purge does. `false` if there was nothing
    /// deleted the caller could see.
    ///
    /// # Errors
    /// `Participants` for the participants of a session or channel, which
    /// go with it, `PermissionDenied`, or whatever the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(deleted) = self.entities.get_deleted_group(ctx, id).await.optional()? else { return Ok(false) };
        if deleted.is_participants() {
            return Err(GroupError::Participants(id));
        }
        let purged = self.entities.purge_group(ctx, id).await?;
        info!(group = %id, by = %ctx.caller_id, "Group purged");
        Ok(purged)
    }

    /// Deletes the groups that lie in `holder` or within it - the
    /// participants of a session or channel among them -, in the runtime's
    /// name (`scoped`), and announces them as `delete` does a team.
    ///
    /// # Errors
    /// Whatever the store reports; what was deleted before stays deleted.
    pub(crate) async fn delete_lying_in(&self, holder: &Scope) -> GroupResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for group in self.entities.list_groups(&system, &within(holder)).await?.items {
            self.entities.delete_group(&system, group.id).await?;
            self.events.publish(GlobalEvent::GroupDeleted(group.id), group.security_attributes);
        }
        Ok(())
    }

    /// Restores the groups that lie in `holder` or within it and were
    /// deleted at `since` or later, in the runtime's name (`scoped`), and
    /// announces them as `restore` does a team. One whose name was taken
    /// meanwhile stays deleted.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    pub(crate) async fn restore_lying_in(&self, holder: &Scope, since: DateTime<Utc>) -> GroupResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for group in deleted_since(self.entities.list_deleted_groups(&system, &within(holder)).await?.items, since) {
            if let Some(restored) = unless_taken(self.entities.restore_group(&system, group.id).await, "group", &group.name)? {
                self.events.publish(GlobalEvent::GroupRestored(restored.id), restored.security_attributes);
            }
        }
        Ok(())
    }

    /// Purges the deleted groups that lie in `holder` or within it, in the
    /// runtime's name (`scoped`).
    ///
    /// # Errors
    /// Whatever the store reports; what was purged before stays purged.
    pub(crate) async fn purge_lying_in(&self, holder: &Scope) -> GroupResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for group in self.entities.list_deleted_groups(&system, &within(holder)).await?.items {
            self.entities.purge_group(&system, group.id).await?;
        }
        Ok(())
    }
}

/// Whether `ctx` may name every group among `entries` that `before` does
/// not name already: it reads the group (`GetGroup`, asked by the store).
/// What a list named before stays, so that its owner keeps a list they
/// change, whoever named a group in it.
///
/// # Errors
/// `StoreError::Validation` naming the first group `ctx` may not read, or
/// whatever else the store reports.
pub(crate) async fn require_nameable(store: &dyn EntityStore, ctx: &SecurityContext, before: &[AclEntry], entries: &[AclEntry]) -> StoreResult<()> {
    for entry in entries {
        let Subject::Group(group) = entry.subject else { continue };
        if before.iter().any(|named| named.subject == entry.subject) {
            continue;
        }
        if store.get_group(ctx, group).await.optional()?.is_none() {
            return Err(StoreError::Validation(format!("an access list names group {group}, which you cannot read")));
        }
    }
    Ok(())
}

/// Stores `participants` as the group of the participants of a session or
/// channel, in the runtime's name: its members replace those stored, and
/// the group is made if there is none yet.
///
/// # Errors
/// Whatever the store reports.
pub(crate) async fn keep_participants(store: &dyn EntityStore, participants: Group) -> StoreResult<()> {
    let system = SecurityContext::system();
    match store.get_group(&system, participants.id).await {
        Ok(mut stored) => {
            if stored.members == participants.members {
                return Ok(());
            }
            stored.members = participants.members;
            stored.updated_at = Utc::now();
            store.update_group(&system, &stored).await
        }
        Err(StoreError::NotFound { .. }) => store.create_group(&system, &participants).await,
        Err(e) => Err(e),
    }
}

/// Removes the group `id` for good, in the runtime's name: marked deleted
/// first if it is not, since only what was deleted is purged (issue 164).
/// For the participants of a channel whose creation failed or that is
/// purged with its session.
///
/// # Errors
/// Whatever the store reports.
pub(crate) async fn purge_participants(store: &dyn EntityStore, id: GroupId) -> StoreResult<bool> {
    let system = SecurityContext::system();
    store.delete_group(&system, id).await?;
    store.purge_group(&system, id).await
}
