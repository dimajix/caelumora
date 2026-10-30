//! `GroupManager`: making, changing, deleting, restoring and purging teams, and deleting and
//! restoring the groups of participants and members with their session, channel or project
//! (issues 286, 348, 467).

use super::{store_error, GroupError, GroupResult};
use crate::event::EventBus;
use caelumora_model::actor::ActorId;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::group::{Group, GroupField, GroupId, GroupUpdateEvent};
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessPolicy, ActionType, Enforcement, SecurityContext, Subject};
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{
    Deleted, EntityStore, EntityStores, StoreError, StoreResult, StoreResultExt, Stored,
};
use caelumora_model::user::{User, UserId};
use caelumora_model::value::name::ScopedName;
use chrono::Utc;
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

/// The one writer of teams (issue 286), and what deletes and restores the groups of the
/// participants of a session or channel, and of the members of a session's own project, with it
/// (issues 348, 467).
pub struct GroupManager {
    users: Arc<dyn EntityStore<User>>,
    groups: Arc<dyn EntityStore<Group>>,
    /// Asked where the manager asks before the store does (issue 312).
    access_policy: Arc<dyn AccessPolicy>,
    events: Arc<EventBus>,
}

impl fmt::Debug for GroupManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupManager").finish_non_exhaustive()
    }
}

impl GroupManager {
    pub(crate) fn new(
        stores: &EntityStores,
        events: Arc<EventBus>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        Self {
            users: Arc::clone(&stores.users),
            groups: Arc::clone(&stores.groups),
            access_policy,
            events,
        }
    }

    /// Makes a team of the user `ctx` acts for, of `name` in `scope` - the
    /// global one or their own -, with nobody in it yet (`CreateGroup`).
    ///
    /// # Errors
    /// `OwnerNotAUser`, `Invalid` for a name that does not fit, or a
    /// `StoreError` - `NameTaken` among them, and `Validation` for a scope a
    /// team does not lie in.
    pub async fn create(
        &self,
        ctx: &SecurityContext,
        name: &str,
        scope: Scope,
    ) -> GroupResult<Group> {
        let ActorId::User(owner) = ctx.principal_id else { return Err(GroupError::OwnerNotAUser) };
        let mut group = Group::team(ScopedName::new(name, scope)?, owner);
        self.groups.create(ctx, &mut group).await?;
        self.events.announce(GlobalEvent::created::<Group>(&group.id), &group);
        info!(group = %group.id, by = %ctx.caller_id, "Group created");
        Ok(group)
    }

    /// Renames team `id` on behalf of `ctx` (`UpdateGroup`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rename(
        &self,
        ctx: &SecurityContext,
        id: GroupId,
        name: &str,
    ) -> GroupResult<Group> {
        self.update(ctx, id, GroupUpdateEvent::NameChanged(name.to_string()), None).await
    }

    /// Makes `members` the members of team `id`, on behalf of `ctx`
    /// (`UpdateGroup`): who is no longer among them no longer reads what was
    /// shared with it.
    ///
    /// # Errors
    /// `UnknownMember` for a user that does not exist, otherwise as
    /// [`Self::update`].
    pub async fn set_members(
        &self,
        ctx: &SecurityContext,
        id: GroupId,
        members: Vec<UserId>,
    ) -> GroupResult<Group> {
        self.update(ctx, id, GroupUpdateEvent::MembersChanged, Some(members)).await
    }

    /// Applies `event` to team `id` on behalf of `ctx` - with `members` for
    /// its new members: stored, which is where the permission is checked
    /// (`UpdateGroup`), then announced to whoever reads it, before and
    /// after. Whether the new members exist is asked of others, in the
    /// runtime's name, so changing the team is asked before (issue 312):
    /// who may not change it learns nothing of who exists. Written
    /// meanwhile elsewhere, the team is read again and changed once more
    /// (issue 492).
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, `NotATeam` for a
    /// group that is no team, `Invalid` if a name does
    /// not fit, `UnknownMember` for a user that does not exist, or whatever
    /// the store reports.
    async fn update(
        &self,
        ctx: &SecurityContext,
        id: GroupId,
        event: GroupUpdateEvent,
        members: Option<Vec<UserId>>,
    ) -> GroupResult<Group> {
        let (group, mut told) = loop {
            let mut group = self.groups.get(ctx, id).await.map_err(store_error(id))?;
            if !group.is_team() {
                return Err(GroupError::NotATeam(id));
            }
            let told: Vec<ActorId> = group.members.clone();
            match &event {
                GroupUpdateEvent::NameChanged(name) => {
                    group.name = ScopedName::new(name, group.name.scope.clone())?
                }
                GroupUpdateEvent::MembersChanged => {}
            }
            if let Some(members) = &members {
                self.access_policy
                    .require(
                        ctx,
                        ActionType::update(Group::KIND, &group.security_attributes),
                        StoreError::PermissionDenied,
                    )
                    .await?;
                let system = SecurityContext::system();
                let mut actors = Vec::with_capacity(members.len());
                for user in members {
                    if self.users.get(&system, *user).await.optional()?.is_none() {
                        return Err(GroupError::UnknownMember(*user));
                    }
                    if !actors.contains(&ActorId::User(*user)) {
                        actors.push(ActorId::User(*user));
                    }
                }
                group.members = actors;
            }
            group.updated_at = Utc::now();
            if self
                .groups
                .update(ctx, &mut group)
                .await
                .written()
                .map_err(store_error(id))?
                .is_some()
            {
                break (group, told);
            }
        };
        // Who left is told too: they read it no longer once it is stored.
        told.retain(|actor| !group.has_member(*actor));
        self.events.announce_also_to(
            GlobalEvent::updated::<Group>(&id, &event),
            &group,
            told.into_iter().map(Subject::Actor),
        );
        info!(group = %id, by = %ctx.caller_id, "Group changed");
        Ok(group)
    }

    /// Deletes team `id` (`DeleteGroup`): marked, kept; who it was shared
    /// with through it reads it no longer. `false` if there was none the
    /// caller could see.
    ///
    /// # Errors
    /// `NotATeam` for a group that is no team, or whatever the store
    /// reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<bool> {
        let Some(group) = self.groups.get(ctx, id).await.optional()? else { return Ok(false) };
        if !group.is_team() {
            return Err(GroupError::NotATeam(id));
        }
        if !self.groups.delete(ctx, id, &Removal::now(ctx.caller_id)).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<Group>(&id), &group);
        info!(group = %id, by = %ctx.caller_id, "Group deleted");
        Ok(true)
    }

    /// Takes the deletion of team `id` back (`Restore`). Returns the team
    /// as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore,
    /// `NotATeam` for a group that is no team, which comes back with what
    /// it belongs to, or whatever else the store reports - `NameTaken`
    /// among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<Group> {
        let Deleted { entity: deleted, .. } =
            self.groups.get_deleted(ctx, id).await.map_err(store_error(id))?;
        if !deleted.is_team() {
            return Err(GroupError::NotATeam(id));
        }
        let group = self.groups.restore(ctx, id).await.map_err(store_error(id))?;
        self.events.announce(GlobalEvent::restored::<Group>(&id), &group);
        info!(group = %id, by = %ctx.caller_id, "Group restored");
        Ok(group)
    }

    /// Removes the deleted team `id` for good (`Purge`, asked by the store).
    /// Announces nothing, as no purge does. `false` if there was nothing
    /// deleted the caller could see.
    ///
    /// # Errors
    /// `NotATeam` for a group that is no team, which goes with what it
    /// belongs to, `PermissionDenied`, or whatever the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: GroupId) -> GroupResult<bool> {
        let Some(Deleted { entity: deleted, .. }) =
            self.groups.get_deleted(ctx, id).await.optional()?
        else {
            return Ok(false);
        };
        if !deleted.is_team() {
            return Err(GroupError::NotATeam(id));
        }
        let purged = self.groups.purge(ctx, id).await?;
        info!(group = %id, by = %ctx.caller_id, "Group purged");
        Ok(purged)
    }
}

/// The groups of the participants of a session or a channel, and of the members of a session's
/// own project, which go and come back with it (issues 348, 467): written in the runtime's name by
/// what deletes and restores the session or channel - the act that was permitted -, as what
/// changes its participants writes them, and announced as a team is (issue 319). Never in the
/// caller's: a session or channel that is there has the group of its participants there as well,
/// whatever the caller may do with groups.
impl GroupManager {
    /// Deletes the groups of the participants that lie in `holders` or within them - of a session,
    /// its channels and its own project's members (issue 467), or of a channel -, as what holds
    /// them is deleted, marked with `removal`, its mark (issue 469): all of them or, if one fails,
    /// none - those deleted before are restored again, so that what holds them, which is then
    /// kept, keeps its members. Announced once all are deleted.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn delete_participants(
        &self,
        holders: &[Scope],
        removal: &Removal,
    ) -> StoreResult<()> {
        let system = SecurityContext::system();
        let mut deleted: Vec<Group> = Vec::new();
        for group in self.groups.list(&system, &lying_in(holders)).await?.items {
            match self.groups.delete(&system, group.id, removal).await {
                Ok(true) => deleted.push(group),
                // Deleted meanwhile by another - with a channel, say -: not this deletion's.
                Ok(false) => {}
                Err(e) => {
                    for group in &deleted {
                        if let Err(undo) = self.groups.restore(&system, group.id).await {
                            warn!(group = %group.id, "The group of participants could not be deleted and not be restored again: {undo}");
                        }
                    }
                    return Err(e);
                }
            }
        }
        for group in deleted {
            self.events.announce(GlobalEvent::deleted::<Group>(&group.id), &group);
        }
        Ok(())
    }

    /// Restores the groups of the participants that lie in `holders` or within them and carry
    /// `removal`, the mark of what holds them - deleted with it, not with a channel deleted before
    /// it (issue 469) -, as that is restored: all of them or, if one fails, none - those restored
    /// before are marked again with `removal`, so that they come back with what holds them once it
    /// is restored. Announced once all are restored.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn restore_participants(
        &self,
        holders: &[Scope],
        removal: &Removal,
    ) -> StoreResult<()> {
        let system = SecurityContext::system();
        let mut restored = Vec::new();
        for group in
            self.groups.list_deleted_with(&system, removal.id, &lying_in(holders)).await?.items
        {
            match self.groups.restore(&system, group.id).await {
                Ok(group) => restored.push(group),
                Err(e) => {
                    for group in &restored {
                        if let Err(undo) = self.groups.delete(&system, group.id, removal).await {
                            warn!(group = %group.id, "The group of participants could not be restored and not be deleted again: {undo}");
                        }
                    }
                    return Err(e);
                }
            }
        }
        for group in restored {
            self.events.announce(GlobalEvent::restored::<Group>(&group.id), &group);
        }
        Ok(())
    }
}

/// What lies in one of `holders` or within it.
fn lying_in(holders: &[Scope]) -> Query<GroupField> {
    Query::filtered(Filter::any_of(holders.iter().map(Filter::lying_within)))
}
