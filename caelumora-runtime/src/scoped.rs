//! What lies in a scope goes with what holds it (issue 178): the personalities, session templates,
//! component bundles, agents, workspaces and skills of a session's scope - and of the scopes of its
//! channels - are deleted with the session, restored with it and purged with it, a workspace with
//! its place (issue 251); those of a channel's scope are deleted and restored with the channel; the
//! artifacts of a project's scope, with their content (issue 271), go with the project - with the
//! session whose own it is (issue 467). `SessionManager`, `SessionLoop` and `ProjectManager` are
//! the callers, through `ScopedEntities`. The group of the participants of a session or channel
//! lies in its scope too, but is not among it: it belongs to the session or channel itself, which
//! deletes and restores it with its own mark (issue 348, `GroupManager::delete_participants`).
//!
//! Each kind that may lie in a scope is a `ScopeMember` (issue 323), kept by `ScopedEntities` in
//! the order it was registered. What is done with what lies in a scope is the same for every kind
//! and written once (`ScopedKind`, issue 461); a member says only what its kind adds. What has an
//! owner of its own - agents, workspaces, skills and artifacts - is a member by its owner (issue
//! 319): what is deleted and restored is announced as it announces its own deletions, and what
//! lies behind an entity - an agent's state, a workspace's place, an artifact's content - is
//! removed by it once the entity's purge is begun, before the entity is purged (issue 492): no
//! restoring takes it back meanwhile. Personalities, session templates and component bundles,
//! which have none, are members by their store (`Unowned`), which marks them alone.
//!
//! Nothing is put into a scope whose holder is gone (issue 458): whoever puts something there asks
//! `ScopeHolders` first that the holder is there and not deleted, and once it is written, asks
//! again. A holder is marked deleted before its scope is gone through, so that who asks afterwards
//! is refused; what was put there before is deleted with it, and what was put there meanwhile -
//! after the scope was gone through - is taken back (`keep_made`, `restore`).
//!
//! What lies in the scope is listed in the runtime's name, so that all of it is found, whatever the
//! caller may read; nothing of the list reaches the caller. It is deleted and restored in the
//! caller's name (issue 320): the store asks each as it asks of anything deleted or restored alone,
//! and marks it with the holder's mark - the one `Removal` of the deletion, by the caller (issue
//! 469). What may lie in a scope is not always its holder's owner's - a contributor's agent in a
//! session's, an artifact another participant published into a project's -, so deleting deletes
//! what the caller may and answers what is left; the holder refuses its deletion if anything is
//! (`ScopeNotEmpty`): it is kept, and what is left is plain to see until its owners delete it.
//! Restoring restores what the caller may and leaves the rest deleted, for its owners to restore:
//! the holder comes back all the same. Purging, the system's or an administrator's, is done in the
//! runtime's name.

use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::Channel;
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::project::Project;
use caelumora_model::removal::Removal;
use caelumora_model::scope::{Scope, Scoped, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::Session;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{
    Deleted, EntityKind, EntityStore, EntityStores, NoEntity, StoreError, StoreResult,
    StoreResultExt, Stored,
};
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
    /// The scope of what was to be deleted: a session's, a channel's or a project's.
    pub holder: Scope,
    /// How many of each kind are left.
    pub left: Vec<(EntityKind, usize)>,
}

impl ScopeNotEmpty {
    /// Refuses the deletion of what holds `holder` if anything is `left`.
    pub(crate) fn require_empty(
        holder: &Scope,
        left: Vec<(EntityKind, usize)>,
    ) -> Result<(), Self> {
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
        Scope::ChannelScope { session_id, channel_id } => {
            format!("channel {channel_id} of session {session_id}")
        }
        Scope::ProjectScope { project_id } => format!("project {project_id}"),
        // Nothing else holds a scope that is deleted with it.
        _ => "holder of the scope".to_string(),
    }
}

/// How many of each kind are left, as an error says it: "1 agent, 2 workspace".
fn shown_left(left: &[(EntityKind, usize)]) -> String {
    left.iter().map(|(kind, count)| format!("{count} {kind}")).collect::<Vec<_>>().join(", ")
}

/// A kind that may lie in a scope and goes with what holds it (issue 178): deleted, restored and
/// purged with the session, channel or project whose scope it lies in. What is done with it there
/// is the same for every kind (`ScopedKind`); a member says only what its kind adds. Implemented by
/// the owner of the kind, or by `Unowned` for a kind without one; a plugin's kind is registered as
/// one (`RuntimeBuilder::scope_member`).
#[async_trait]
pub trait ScopeMember: Send + Sync {
    /// The kind of entity it is: one that lies in a scope, and in no other entity.
    type Entity: Stored<Within = NoEntity, Field: ScopedField> + Scoped;

    /// The store its entities are kept in.
    fn store(&self) -> &dyn EntityStore<Self::Entity>;

    /// Where what is deleted and restored is announced, as its owner announces its own deletions
    /// and restorings; `None`, the default, announces nothing.
    fn events(&self) -> Option<&dyn EventPublisher> {
        None
    }

    /// Removes what lies behind `entity` - an agent's state, a workspace's place, an artifact's
    /// content -, whose purge is begun (`EntityStore::begin_purge`). It is removed before the
    /// entity is purged, so that if it fails, the entity stays deleted and is purged when its
    /// holder is purged again. Nothing, by default.
    ///
    /// # Errors
    /// Whatever the store reports; as `StoreError::Backend` a failure to remove what lies behind
    /// it, which is its storage too.
    async fn remove_behind(&self, _entity: &Self::Entity) -> StoreResult<()> {
        Ok(())
    }
}

/// A `ScopeMember` as `ScopedEntities` goes through it, whatever its kind: what of the kind lies in
/// a scope is listed in the runtime's name, deleted and restored in the caller's and purged in the
/// runtime's - the same for every kind, with what its member adds.
#[async_trait]
pub(crate) trait ScopedKind: Send + Sync {
    /// The kind this member deletes, restores and purges.
    fn kind(&self) -> EntityKind;

    /// Deletes what of its kind lies in `holder` or within it on behalf of `ctx`, as far as they
    /// may, marked with `removal`, the holder's mark (issue 469), and announces it; answers how
    /// many are left.
    ///
    /// # Errors
    /// Whatever else the store reports; what was deleted before stays
    /// deleted.
    async fn delete_lying_in(
        &self,
        ctx: &SecurityContext,
        holder: &Scope,
        removal: &Removal,
    ) -> StoreResult<usize>;

    /// Restores what of its kind lies in `holder` or within it and carries `removal`, the holder's
    /// mark - what was deleted with it (issue 469) -, on behalf of `ctx`, and announces it. One
    /// whose name was taken meanwhile stays deleted, as does one `ctx` may not restore.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    async fn restore_lying_in(
        &self,
        ctx: &SecurityContext,
        holder: &Scope,
        removal: &Removal,
    ) -> StoreResult<()>;

    /// Purges what of its kind lies in `holder` or within it and is deleted, each after what lies
    /// behind it (`ScopeMember::remove_behind`), in the runtime's name (`purge_behind`).
    ///
    /// # Errors
    /// Whatever `remove_behind` and the store report; what was purged before stays purged.
    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()>;
}

#[async_trait]
impl<M: ScopeMember> ScopedKind for M {
    fn kind(&self) -> EntityKind {
        M::Entity::KIND
    }

    async fn delete_lying_in(
        &self,
        ctx: &SecurityContext,
        holder: &Scope,
        removal: &Removal,
    ) -> StoreResult<usize> {
        let store = self.store();
        let mut left = 0;
        for entity in store
            .list(&SecurityContext::system(), &Query::filtered(Filter::lying_within(holder)))
            .await?
            .items
        {
            if deleted_unless_refused(store.delete(ctx, entity.id(), removal).await)? {
                if let Some(events) = self.events() {
                    events.announce(GlobalEvent::deleted::<M::Entity>(&entity.id()), &entity);
                }
            } else if store.get(&SecurityContext::system(), entity.id()).await.optional()?.is_some()
            {
                // Left only if it is still there: one deleted meanwhile by another is gone.
                left += 1;
            }
        }
        Ok(left)
    }

    async fn restore_lying_in(
        &self,
        ctx: &SecurityContext,
        holder: &Scope,
        removal: &Removal,
    ) -> StoreResult<()> {
        let store = self.store();
        for entity in store
            .list_deleted_with(
                &SecurityContext::system(),
                removal.id,
                &Query::filtered(Filter::lying_within(holder)),
            )
            .await?
            .items
        {
            if let Some(restored) =
                restored_unless_refused(store.restore(ctx, entity.id()).await, &entity)?
            {
                if let Some(events) = self.events() {
                    events.announce(GlobalEvent::restored::<M::Entity>(&restored.id()), &restored);
                }
            }
        }
        Ok(())
    }

    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        for Deleted { entity, .. } in self
            .store()
            .list_deleted(&system, &Query::filtered(Filter::lying_within(holder)))
            .await?
            .items
        {
            purge_behind(self, entity.id()).await?;
        }
        Ok(())
    }
}

/// Purges the deleted entity `id` of `member`'s store in the runtime's name, what lies behind it
/// first: its purge begun, so that no restoring takes it back while that goes (issue 492), then
/// what lies behind it removed (`ScopeMember::remove_behind`), then the entity. `false` if there
/// was none deleted - `keep_made` then removes what lies behind it alone.
///
/// # Errors
/// Whatever `remove_behind` and the store report; the entity then stays deleted, its purge begun.
async fn purge_behind<M: ScopeMember>(
    member: &M,
    id: <M::Entity as Stored>::Id,
) -> StoreResult<bool> {
    let system = SecurityContext::system();
    let store = member.store();
    let Some(entity) = store.begin_purge(&system, id.clone()).await? else { return Ok(false) };
    member.remove_behind(&entity).await?;
    store.purge(&system, id).await
}

/// Deletes, restores and purges what lies in a scope with what holds it
/// (issue 178), each kind by its `ScopeMember`, in the order they were
/// registered.
pub(crate) struct ScopedEntities {
    members: Vec<Arc<dyn ScopedKind>>,
}

impl fmt::Debug for ScopedEntities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedEntities").finish_non_exhaustive()
    }
}

impl ScopedEntities {
    /// Goes through `members` in their order.
    pub(crate) fn new(members: Vec<Arc<dyn ScopedKind>>) -> Self {
        Self { members }
    }

    /// Deletes what lies in `holders` or within them - the scopes of what is deleted: a session's
    /// and its own project's (issue 467), or a channel's - on behalf of `ctx`, as far as they may
    /// delete it, marked with `removal`, the mark of what holds them (issue 469), and announces it
    /// as its owner announces a deletion. Answers what is left, by kind, counted over all of them -
    /// empty if nothing is -, which the holder refuses its deletion for (`ScopeNotEmpty`).
    ///
    /// Called once the holder is marked deleted, so that nothing goes unless the holder's deletion
    /// was granted, and nothing reaches the holder while its scope goes; what goes carries its mark
    /// and comes back with it (`restore_lying_in`). If something is left, the caller takes the mark
    /// away again.
    ///
    /// # Errors
    /// Whatever the store reports; what was deleted before stays deleted.
    pub(crate) async fn delete_lying_in(
        &self,
        ctx: &SecurityContext,
        holders: &[Scope],
        removal: &Removal,
    ) -> StoreResult<Vec<(EntityKind, usize)>> {
        let mut left = Vec::new();
        for member in &self.members {
            let mut count = 0;
            for holder in holders {
                count += member.delete_lying_in(ctx, holder, removal).await?;
            }
            if count > 0 {
                left.push((member.kind(), count));
            }
        }
        Ok(left)
    }

    /// Restores what lies in `holders` or within them and was deleted with them - what carries
    /// `removal`, the mark of what holds them (issue 469) - on behalf of `ctx`, and announces it as
    /// its owner announces a restoring. What was deleted on its own stays deleted, as a channel
    /// deleted before its session does; so does one whose name was taken meanwhile (issue 250) and
    /// one `ctx` may not restore (issue 320), and the others are restored all the same.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays restored.
    pub(crate) async fn restore_lying_in(
        &self,
        ctx: &SecurityContext,
        holders: &[Scope],
        removal: &Removal,
    ) -> StoreResult<()> {
        for member in &self.members {
            for holder in holders {
                member.restore_lying_in(ctx, holder, removal).await?;
            }
        }
        Ok(())
    }

    /// Purges what lies in `holders` or within them and is deleted, with what its owner purges with
    /// it: the agents' state, the workspaces' places, the artifacts' content.
    ///
    /// # Errors
    /// Whatever the store reports, a place or a content that cannot be removed among it
    /// (`ScopeMember::remove_behind`); what was purged before stays purged.
    pub(crate) async fn purge_lying_in(&self, holders: &[Scope]) -> StoreResult<()> {
        for member in &self.members {
            for holder in holders {
                member.purge_lying_in(holder).await?;
            }
        }
        Ok(())
    }
}

/// A kind without an owner of its own - personalities, session templates and component bundles - as
/// a `ScopeMember`: marked and purged in its store alone, with nothing announced.
pub(crate) struct Unowned<T: Stored>(pub(crate) Arc<dyn EntityStore<T>>);

impl<T> ScopeMember for Unowned<T>
where
    T: Stored<Within = NoEntity, Field: ScopedField> + Scoped,
{
    type Entity = T;

    fn store(&self) -> &dyn EntityStore<T> {
        self.0.as_ref()
    }
}

/// The holders of scopes - sessions, channels, projects -: whether one is there for whoever puts
/// something into its scope (issue 458). Asked before anything is written (`require`) and once it
/// is (`keep_made`, `restore`): a holder is marked deleted before its scope is gone through
/// (`ScopedEntities::delete_lying_in`), so that who asks afterwards is refused, and what was
/// written meanwhile is taken back.
pub struct ScopeHolders {
    sessions: Arc<dyn EntityStore<Session>>,
    channels: Arc<dyn EntityStore<Channel>>,
    projects: Arc<dyn EntityStore<Project>>,
}

impl ScopeHolders {
    /// The holders as `stores` keep them.
    pub(crate) fn new(stores: &EntityStores) -> Self {
        Self {
            sessions: Arc::clone(&stores.sessions),
            channels: Arc::clone(&stores.channels),
            projects: Arc::clone(&stores.projects),
        }
    }

    /// Requires that `ctx` may put something into `scope`: they read its holder - a session, a
    /// channel, a project -, and it is not deleted. A channel's session is asked after the channel,
    /// in the runtime's name, since a channel is not marked with its session - after, so that who
    /// may not read the channel learns nothing of the session; a scope without a holder - a user's,
    /// a group's, the global one - asks nothing. Asked whatever the kind, also of a scope the kind
    /// cannot lie in, which the store then refuses (`Stored::validate`).
    ///
    /// # Errors
    /// `StoreError::NotFound` for a holder `ctx` may not read or that is deleted, or a channel of
    /// another session than the scope names.
    pub async fn require(&self, ctx: &SecurityContext, scope: &Scope) -> StoreResult<()> {
        match scope {
            Scope::SessionScope { session_id } => {
                self.sessions.get(ctx, *session_id).await?;
            }
            Scope::ChannelScope { session_id, channel_id } => {
                if self.channels.get(ctx, *channel_id).await?.session_id != *session_id {
                    return Err(StoreError::NotFound {
                        entity: Channel::KIND.name(),
                        id: channel_id.to_string(),
                    });
                }
                self.sessions.get(&SecurityContext::system(), *session_id).await?;
            }
            Scope::ProjectScope { project_id } => {
                self.projects.get(ctx, *project_id).await?;
            }
            Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::GlobalScope => {}
        }
        Ok(())
    }

    /// Keeps `entity`, made by `member`'s store a moment ago, if the holder of its scope is still
    /// there - asked in the runtime's name -; takes it back otherwise: made after the holder's
    /// deletion went through the scope, it would be left behind. Taken back as a holder's purge
    /// takes it: marked deleted, then purged with what lies behind it (`purge_behind`) - so that
    /// if removing fails, it stays deleted and goes with the holder's purge. Purged meanwhile with
    /// the holder, before what lies behind it was made, what lies behind it is removed alone.
    ///
    /// # Errors
    /// `StoreError::NotFound` for the holder if it went, or whatever taking back reports.
    pub async fn keep_made<M: ScopeMember>(
        &self,
        member: &M,
        entity: &M::Entity,
    ) -> StoreResult<()> {
        let Err(gone) = self.still_there(entity).await else { return Ok(()) };
        info!(kind = %M::Entity::KIND, id = %entity.id(), "Taken back: what holds its scope went while it was made");
        let system = SecurityContext::system();
        member.store().delete(&system, entity.id(), &Removal::now(ActorId::System)).await?;
        if !purge_behind(member, entity.id()).await? {
            member.remove_behind(entity).await?;
        }
        Err(gone)
    }

    /// Restores `id` of `member`'s store on behalf of `ctx`, as the store restores it, and keeps it
    /// only if the holder of its scope is still there (`keep_restored`): otherwise it is marked
    /// deleted again with the mark it carried - read in the runtime's name before, since the mark
    /// is the runtime's to give back; the store asks `ctx` as it restores. Answers the entity as
    /// restored.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted entity, `ctx` may not see it, its purge
    /// is begun (issue 492), or the holder of its scope is gone; whatever else the store reports -
    /// `NameTaken` among it.
    pub async fn restore<M: ScopeMember>(
        &self,
        member: &M,
        ctx: &SecurityContext,
        id: <M::Entity as Stored>::Id,
    ) -> StoreResult<M::Entity> {
        let store = member.store();
        let Deleted { removal, .. } =
            store.get_deleted(&SecurityContext::system(), id.clone()).await?;
        let entity = store.restore(ctx, id).await?;
        self.keep_restored(member, &entity, &removal).await?;
        Ok(entity)
    }

    /// Keeps `entity`, restored by `member`'s store a moment ago, if the holder of its scope is
    /// still there - asked in the runtime's name -; marks it deleted again with `removal`, the mark
    /// it carried, otherwise.
    ///
    /// # Errors
    /// `StoreError::NotFound` for the holder if it went, or whatever marking reports.
    async fn keep_restored<M: ScopeMember>(
        &self,
        member: &M,
        entity: &M::Entity,
        removal: &Removal,
    ) -> StoreResult<()> {
        let Err(gone) = self.still_there(entity).await else { return Ok(()) };
        info!(kind = %M::Entity::KIND, id = %entity.id(), "Stays deleted: what holds its scope went while it was restored");
        member.store().delete(&SecurityContext::system(), entity.id(), removal).await?;
        Err(gone)
    }

    /// Whether the holder of the scope `entity` lies in is still there, asked in the runtime's
    /// name: `Err(NotFound)` if it went. Any other failure is the store's and says nothing of the
    /// holder: logged, and the entity kept.
    async fn still_there<T: Scoped>(&self, entity: &T) -> StoreResult<()> {
        match self.require(&SecurityContext::system(), &entity.scoped_name().scope).await {
            Err(gone @ StoreError::NotFound { .. }) => Err(gone),
            Err(e) => {
                warn!("Whether what holds a scope is still there could not be asked: {e}");
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}

/// Whether `deleted` says it was deleted; `false` also if it was not the
/// caller's to delete (issue 320), which leaves it.
fn deleted_unless_refused(deleted: StoreResult<bool>) -> StoreResult<bool> {
    match deleted {
        Err(StoreError::PermissionDenied(_)) => Ok(false),
        deleted => deleted,
    }
}

/// What `restored` says was restored of `deleted`; `None` if it stays
/// deleted: its name was taken meanwhile, it is not the caller's to
/// restore (issue 320), or its purge is begun (issue 492).
fn restored_unless_refused<D: Stored, T>(
    restored: StoreResult<T>,
    deleted: &D,
) -> StoreResult<Option<T>> {
    match restored {
        Ok(value) => Ok(Some(value)),
        Err(StoreError::NameTaken { .. }) => {
            warn!(kind = %D::KIND, id = %deleted.id(), "Stays deleted: its name was taken while what holds its scope was deleted");
            Ok(None)
        }
        Err(e) if e.is_absent_or_refused() => {
            info!(kind = %D::KIND, id = %deleted.id(), "Stays deleted: it is not the caller's to restore, or its purge is begun");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Removes `id` of `store` for good, in the runtime's name: marked deleted first if it is not,
/// since only what was deleted is purged (issue 164) - for what goes with something purged, or was
/// made for something whose making failed. `false` if there is none.
///
/// # Errors
/// Whatever else the store reports.
pub(crate) async fn purge_for_good<T: Stored>(
    store: &dyn EntityStore<T>,
    id: T::Id,
) -> StoreResult<bool> {
    let system = SecurityContext::system();
    if store.delete(&system, id.clone(), &Removal::now(ActorId::System)).await.optional()?.is_none()
    {
        return Ok(false);
    }
    store.purge(&system, id).await
}

#[cfg(test)]
mod tests {
    use super::{ScopeHolders, Unowned};
    use caelumora_model::actor::ActorId;
    use caelumora_model::channel::testing::make_channel;
    use caelumora_model::component::ComponentBundle;
    use caelumora_model::project::Project;
    use caelumora_model::removal::Removal;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{SecurityAttributes, SecurityContext};
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::SessionId;
    use caelumora_model::store::{Persistence, StoreError};
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;
    use std::sync::Arc;

    /// Whether `holders` refuse `scope` for want of its holder.
    async fn refused(holders: &ScopeHolders, scope: &Scope) -> bool {
        let asked = holders.require(&SecurityContext::system(), scope).await;
        matches!(asked, Err(StoreError::NotFound { .. }))
    }

    /// Issue 458: something is put into a scope only while its holder is there - a channel's
    /// session among it, which is not marked with the session.
    #[tokio::test]
    async fn a_scope_is_entered_only_while_its_holder_is_there() {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let holders = ScopeHolders::new(&stores);
        let system = SecurityContext::system();
        let mut session = bare_session(SessionId::new());
        let mut channel = make_channel(session.id, "main");
        let mut project = Project::own_of(session.id, UserId::new());
        stores.sessions.create(&system, &mut session).await.unwrap();
        stores.channels.create(&system, &mut channel).await.unwrap();
        stores.projects.create(&system, &mut project).await.unwrap();
        let in_session = Scope::SessionScope { session_id: session.id };
        let in_channel = Scope::ChannelScope { session_id: session.id, channel_id: channel.id };
        let in_project = Scope::ProjectScope { project_id: project.id };
        let elsewhere =
            Scope::ChannelScope { session_id: SessionId::new(), channel_id: channel.id };

        for scope in [&in_session, &in_channel, &in_project, &Scope::GlobalScope] {
            holders.require(&system, scope).await.expect("its holder is there");
        }
        assert!(refused(&holders, &elsewhere).await, "a channel of another session is none");

        let mark = Removal::now(ActorId::System);
        stores.projects.delete(&system, project.id, &mark).await.unwrap();
        assert!(refused(&holders, &in_project).await, "a deleted project holds nothing new");
        stores.sessions.delete(&system, session.id, &mark).await.unwrap();
        assert!(refused(&holders, &in_session).await, "nor a deleted session");
        assert!(refused(&holders, &in_channel).await, "nor a channel of a deleted session");
        holders
            .require(&system, &Scope::GlobalScope)
            .await
            .expect("the global scope has no holder");
    }

    /// A bundle `name` in `scope`, the system's.
    fn bundle(name: &str, scope: &Scope) -> ComponentBundle {
        ComponentBundle::new(
            ScopedName::new(name, scope.clone()).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        )
    }

    /// Issue 458: what was put into a scope after its holder's deletion went through it is taken
    /// back - made, purged; restored, marked deleted again with the mark it carried.
    #[tokio::test]
    async fn what_is_put_into_a_scope_whose_holder_went_is_taken_back() {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let holders = ScopeHolders::new(&stores);
        let bundles = Unowned(Arc::clone(&stores.component_bundles));
        let system = SecurityContext::system();
        let mut session = bare_session(SessionId::new());
        stores.sessions.create(&system, &mut session).await.unwrap();
        let in_session = Scope::SessionScope { session_id: session.id };

        let mut kept = bundle("kept", &in_session);
        stores.component_bundles.create(&system, &mut kept).await.unwrap();
        holders.keep_made(&bundles, &kept).await.expect("its session is there");
        let alone = Removal::now(ActorId::System);
        stores.component_bundles.delete(&system, kept.id, &alone).await.unwrap();
        holders.restore(&bundles, &system, kept.id).await.expect("still there");

        // The session goes, and its scope was gone through: `kept` went with it.
        let mark = Removal::now(ActorId::System);
        stores.sessions.delete(&system, session.id, &mark).await.unwrap();
        stores.component_bundles.delete(&system, kept.id, &mark).await.unwrap();
        let mut late = bundle("late", &in_session);
        stores.component_bundles.create(&system, &mut late).await.unwrap();
        let refused = holders.keep_made(&bundles, &late).await;
        assert!(matches!(refused, Err(StoreError::NotFound { .. })), "made too late: {refused:?}");
        assert!(
            stores.component_bundles.get_deleted(&system, late.id).await.is_err(),
            "taken back for good"
        );
        let refused = holders.restore(&bundles, &system, kept.id).await;
        assert!(
            matches!(refused, Err(StoreError::NotFound { .. })),
            "restored too late: {refused:?}"
        );
        assert_eq!(
            stores.component_bundles.get_deleted(&system, kept.id).await.unwrap().removal,
            mark,
            "stays deleted with the session, as it was"
        );
    }
}
