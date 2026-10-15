//! `GroupManager`: making, changing, deleting, restoring and purging teams,
//! and deleting and restoring the groups of participants with their
//! session or channel (issues 286, 348).

use super::{store_error, GroupError, GroupResult};
use crate::event::EventBus;
use crate::scoped::{deleted_since, within};
use caelumora_model::actor::ActorId;
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId, GroupUpdateEvent};
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, Grant, SecurityContext, SecurityManager, Subject};
use caelumora_model::store::{EntityStore, EntityStores, StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_model::user::{User, UserId};
use caelumora_model::value::name::ScopedName;
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// The one writer of teams (issue 286), and what deletes and restores
/// the groups of the participants of a session or channel with it (issue
/// 348).
pub struct GroupManager {
    users: Arc<dyn EntityStore<User>>,
    groups: Arc<dyn EntityStore<Group>>,
    /// Asked where the manager asks before the store does (issue 312).
    security: Arc<dyn SecurityManager>,
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
    pub(crate) fn new(stores: &EntityStores, events: Arc<EventBus>, security: Arc<dyn SecurityManager>) -> Self {
        Self { users: Arc::clone(&stores.users), groups: Arc::clone(&stores.groups), security, events, writing: Mutex::new(()) }
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
        self.groups.create(ctx, &group).await?;
        self.events.publish(GlobalEvent::created::<Group>(&group.id), group.security_attributes.clone());
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
        self.update(ctx, id, GroupUpdateEvent::MembersChanged, Some(members)).await
    }

    /// Applies `event` to team `id` on behalf of `ctx` - with `members` for
    /// its new members: stored, which is where the permission is checked
    /// (`UpdateGroup`), then announced to whoever reads it, before and
    /// after. Whether the new members exist is asked of others, in the
    /// runtime's name, so changing the team is asked before (issue 312):
    /// who may not change it learns nothing of who exists.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, `Participants` for
    /// the participants of a session or channel, `Invalid` if a name does
    /// not fit, `UnknownMember` for a user that does not exist, or whatever
    /// the store reports.
    async fn update(&self, ctx: &SecurityContext, id: GroupId, event: GroupUpdateEvent, members: Option<Vec<UserId>>) -> GroupResult<Group> {
        let _writing = self.writing.lock().await;
        let mut group = self.groups.get(ctx, id).await.map_err(store_error(id))?;
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
            self.security.require(ctx, ActionType::update(Group::KIND, &group.security_attributes), StoreError::PermissionDenied).await?;
            let system = SecurityContext::system();
            let mut actors = Vec::with_capacity(members.len());
            for user in members {
                if self.users.get(&system, user).await.optional()?.is_none() {
                    return Err(GroupError::UnknownMember(user));
                }
                if !actors.contains(&ActorId::User(user)) {
                    actors.push(ActorId::User(user));
                }
            }
            group.members = actors;
        }
        group.updated_at = Utc::now();
        self.groups.update(ctx, &group).await.map_err(store_error(id))?;
        // Who left is told too: they read it no longer once it is stored.
        told.retain(|actor| !group.has_member(*actor));
        let attributes = told.into_iter().fold(before, |attributes, actor| attributes.granting(Subject::Actor(actor), Grant::Read));
        self.events.publish(GlobalEvent::updated::<Group>(&id, &event), attributes);
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
        let Some(group) = self.groups.get(ctx, id).await.optional()? else { return Ok(false) };
        if group.is_participants() {
            return Err(GroupError::Participants(id));
        }
        if !self.groups.delete(ctx, id).await? {
            return Ok(false);
        }
        self.events.publish(GlobalEvent::deleted::<Group>(&id), group.security_attributes);
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
        let deleted = self.groups.get_deleted(ctx, id).await.map_err(store_error(id))?;
        if deleted.is_participants() {
            return Err(GroupError::Participants(id));
        }
        let group = self.groups.restore(ctx, id).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::restored::<Group>(&id), group.security_attributes.clone());
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
        let Some(deleted) = self.groups.get_deleted(ctx, id).await.optional()? else { return Ok(false) };
        if deleted.is_participants() {
            return Err(GroupError::Participants(id));
        }
        let purged = self.groups.purge(ctx, id).await?;
        info!(group = %id, by = %ctx.caller_id, "Group purged");
        Ok(purged)
    }
}

/// The groups of the participants of a session or a channel, which go and
/// come back with it (issue 348): written in the runtime's name by what
/// deletes and restores the session or channel - the act that was
/// permitted -, as what changes its participants writes them, and announced
/// as a team is (issue 319). Never in the caller's: a session or channel
/// that is there has the group of its participants there as well, whatever
/// the caller may do with groups.
impl GroupManager {
    /// Deletes the groups of the participants that lie in `holder` or
    /// within it - of a session and its channels, or of a channel -, as
    /// `holder` is deleted: all of them or, if one fails, none - those
    /// deleted before are restored again, so that `holder`, which is then
    /// kept, keeps its members. Announced once all are deleted.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn delete_participants(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        let mut deleted: Vec<Group> = Vec::new();
        for group in self.groups.list(&system, &within(holder)).await?.items {
            if let Err(e) = self.groups.delete(&system, group.id).await {
                for group in &deleted {
                    if let Err(undo) = self.groups.restore(&system, group.id).await {
                        warn!(group = %group.id, "The group of participants could not be deleted and not be restored again: {undo}");
                    }
                }
                return Err(e);
            }
            deleted.push(group);
        }
        for group in deleted {
            self.events.publish(GlobalEvent::deleted::<Group>(&group.id), group.security_attributes);
        }
        Ok(())
    }

    /// Restores the groups of the participants that lie in `holder` or
    /// within it and were deleted at `since` or later - with `holder`, not
    /// with a channel deleted before it -, as `holder` is restored: all of
    /// them or, if one fails, none - those restored before are deleted
    /// again, so that `holder`, which is then deleted again, takes them
    /// along. Announced once all are restored.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn restore_participants(&self, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        let mut restored = Vec::new();
        for group in deleted_since(self.groups.list_deleted(&system, &within(holder)).await?.items, since) {
            match self.groups.restore(&system, group.id).await {
                Ok(group) => restored.push(group),
                Err(e) => {
                    for group in &restored {
                        if let Err(undo) = self.groups.delete(&system, group.id).await {
                            warn!(group = %group.id, "The group of participants could not be restored and not be deleted again: {undo}");
                        }
                    }
                    return Err(e);
                }
            }
        }
        for group in restored {
            self.events.publish(GlobalEvent::restored::<Group>(&group.id), group.security_attributes);
        }
        Ok(())
    }
}
