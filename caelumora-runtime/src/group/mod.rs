//! Groups in the runtime (issues 286, 304): the teams users make, and the
//! groups that sessions, channels and projects keep of who belongs to
//! them.
//!
//! The participants of a session or a channel are a group an access list
//! names - the session's and the channel's own, a workspace attached to the
//! session, an artifact published in the channel. Its one writer is what
//! changes the participants: the `SessionManager` and `SessionLoop` as they
//! make a session or a channel, the `ChannelController` as someone joins or
//! leaves. The group is written in the runtime's name after the change of
//! the participants it follows, which is the act that was permitted; it is
//! derived anew from the participants each time, so that a write that
//! failed is corrected by the next. The group goes and comes back with its
//! session or channel, not with what lies in its scope (issue 348): what
//! deletes, restores and purges the session or channel deletes, restores
//! and purges the group with it, in the runtime's name as well - the
//! `GroupManager` deletes and restores it, announcing it as a team
//! (`…_participants`) -, so that a session or channel that is there has
//! its group there, too. The members of a session's own project are a group
//! kept the same way, made, deleted, restored and purged with the project by
//! the `SessionManager` (issue 467).
//!
//! A team is made and changed by the `GroupManager`, its one writer, which
//! leaves the permissions to the store - who reads, changes and deletes a
//! group is the policy's (`GetGroup`, `UpdateGroup`, `DeleteGroup`); the
//! participants of a session or channel it does not change by hand at all.
//! Reading groups is the views' (`view::GroupView`).
//!
//! Whoever names a group in an access list must be able to read it
//! (`require_nameable`): naming one grants its members something, and nobody
//! grants what they cannot tell to whom. An entry naming a group that is
//! gone stays until the list is stored again, which drops it
//! (`prune_gone_groups`, issue 347).

mod manager;

pub use manager::GroupManager;

use caelumora_model::group::{Group, GroupId};
use caelumora_model::security::{AclEntry, SecurityContext, Subject};
use caelumora_model::store::{EntityStore, StoreError, StoreResult, StoreResultExt};
use caelumora_model::user::UserId;
use caelumora_model::value::ValueError;
use chrono::Utc;
use tracing::{debug, warn};

#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    /// No such group, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no group {0}")]
    NotFound(GroupId),
    #[error("a team belongs to a user")]
    OwnerNotAUser,
    /// A group that is no team - the participants of a session or a channel,
    /// the members of a project - changes with what it belongs to, not by
    /// hand.
    #[error("group {0} is no team: it changes with the session, channel or project it belongs to")]
    NotATeam(GroupId),
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

/// Whether `ctx` may name every group among `entries` that `before` does
/// not name already: it reads the group (`GetGroup`, asked by the store).
/// What a list named before stays, so that its owner keeps a list they
/// change, whoever named a group in it.
///
/// # Errors
/// `StoreError::Validation` naming the first group `ctx` may not read, or
/// whatever else the store reports.
pub async fn require_nameable(
    store: &dyn EntityStore<Group>,
    ctx: &SecurityContext,
    before: &[AclEntry],
    entries: &[AclEntry],
) -> StoreResult<()> {
    for entry in entries {
        let Subject::Group(group) = entry.subject else { continue };
        if before.iter().any(|named| named.subject == entry.subject) {
            continue;
        }
        if store.get(ctx, group).await.optional()?.is_none() {
            return Err(StoreError::Validation(format!(
                "an access list names group {group}, which you cannot read"
            )));
        }
    }
    Ok(())
}

/// Drops from `lists` - the access lists of one entity - the entries that
/// name a group which is gone (issue 347): purged, neither there nor
/// deleted, as the runtime finds it. Called wherever such an entity is
/// stored, so that an entry for a session long purged goes as the list is
/// next written, not as the group is purged: that would have to find every
/// list naming it, of every kind. A deleted group comes back with
/// what it belongs to and keeps its entries, and so does one that cannot be
/// looked up: an entry is dropped only for certain.
pub async fn prune_gone_groups<'a>(
    store: &dyn EntityStore<Group>,
    lists: impl IntoIterator<Item = &'a mut Vec<AclEntry>>,
) {
    let lists: Vec<&mut Vec<AclEntry>> = lists.into_iter().collect();
    let gone = gone_groups(store, lists.iter().map(|list| &**list)).await;
    prune_groups(lists, &gone);
}

/// The groups named by `lists` that are gone for certain (`is_gone`): what `prune_groups` drops
/// from a list, told apart before a change that is written again when it was refused as outdated
/// (issue 492).
pub(crate) async fn gone_groups<'a>(
    store: &dyn EntityStore<Group>,
    lists: impl IntoIterator<Item = &'a Vec<AclEntry>>,
) -> Vec<GroupId> {
    let mut named = Vec::new();
    for entry in lists.into_iter().flat_map(|list| list.iter()) {
        match entry.subject {
            Subject::Group(group) if !named.contains(&group) => named.push(group),
            _ => {}
        }
    }
    let mut gone = Vec::new();
    for group in named {
        if is_gone(store, group).await {
            gone.push(group);
        }
    }
    gone
}

/// Drops from `lists` every entry naming one of `gone`, the groups `gone_groups` told apart.
pub(crate) fn prune_groups<'a>(
    lists: impl IntoIterator<Item = &'a mut Vec<AclEntry>>,
    gone: &[GroupId],
) {
    if gone.is_empty() {
        return;
    }
    for list in lists {
        list.retain(
            |entry| !matches!(entry.subject, Subject::Group(group) if gone.contains(&group)),
        );
    }
    debug!(groups = ?gone, "Entries naming groups that are gone dropped from an access list");
}

/// Whether group `id` is neither there nor deleted, looked for in the
/// runtime's name; `false` if that cannot be told.
async fn is_gone(store: &dyn EntityStore<Group>, id: GroupId) -> bool {
    let system = SecurityContext::system();
    let found = match store.get(&system, id).await.optional() {
        Ok(None) => {
            store.get_deleted(&system, id).await.optional().map(|deleted| deleted.is_some())
        }
        there => there.map(|live| live.is_some()),
    };
    found.map(|found| !found).unwrap_or_else(|e| {
        warn!(group = %id, "Cannot tell whether group is gone; the entries naming it stay: {e}");
        false
    })
}

/// Stores `participants` as the group of the participants of a session or channel - or of the
/// members of a project (issue 467) -, in the runtime's name: its members replace those stored, and
/// the group is made if there is none yet. Called only while the session, channel or project is
/// there, so a group found deleted is restored first (issue 347): it goes and comes back with it
/// (issue 348), but taking back a deletion that failed may have failed, too. Written meanwhile
/// elsewhere, the group is read again and the members set once more (issue 492).
///
/// # Errors
/// Whatever the store reports.
pub(crate) async fn keep_participants(
    store: &dyn EntityStore<Group>,
    participants: Group,
) -> StoreResult<()> {
    let system = SecurityContext::system();
    loop {
        let mut stored = match store.get(&system, participants.id).await.optional()? {
            Some(stored) => stored,
            None => match store.get_deleted(&system, participants.id).await.optional()? {
                Some(_) => store.restore(&system, participants.id).await?,
                None => return store.create(&system, &participants).await,
            },
        };
        if stored.members == participants.members {
            return Ok(());
        }
        stored.members = participants.members.clone();
        stored.updated_at = Utc::now();
        if store.update(&system, &mut stored).await.written()?.is_some() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;
    use async_trait::async_trait;
    use caelumora_model::actor::ActorId;
    use caelumora_model::channel::ChannelId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::group::GroupField;
    use caelumora_model::project::Project;
    use caelumora_model::removal::{Removal, RemovalId};
    use caelumora_model::scope::Scope;
    use caelumora_model::security::Grant;
    use caelumora_model::security::{AllowAllPolicy, SecurityAttributes};
    use caelumora_model::session::SessionId;
    use caelumora_model::store::documents::Document;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::query::{Page, Query};
    use caelumora_model::store::testing::ContendedStore;
    use caelumora_model::store::EntityStores;
    use caelumora_model::store::Persistence;
    use caelumora_model::store::{Deleted, PersistenceBackend, Stored};
    use caelumora_model::value::name::ScopedName;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A store of groups whose deleting and restoring fail once, after as
    /// many as `passing` let through - counted down past zero, never again;
    /// otherwise as `inner`.
    #[derive(Debug)]
    struct Failing {
        inner: Arc<dyn EntityStore<Group>>,
        passing: AtomicUsize,
    }

    impl Failing {
        /// Whether the deleting or restoring now is the one to fail.
        fn fails(&self) -> StoreResult<()> {
            match self.passing.fetch_sub(1, Ordering::SeqCst) {
                0 => Err(StoreError::Backend("failing".to_string())),
                _ => Ok(()),
            }
        }
    }

    #[async_trait]
    impl EntityStore<Group> for Failing {
        async fn create(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()> {
            self.inner.create(ctx, value).await
        }
        async fn get(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
            self.inner.get(ctx, id).await
        }
        async fn update(&self, ctx: &SecurityContext, value: &mut Group) -> StoreResult<()> {
            self.inner.update(ctx, value).await
        }
        async fn delete(
            &self,
            ctx: &SecurityContext,
            id: GroupId,
            removal: &Removal,
        ) -> StoreResult<bool> {
            self.fails()?;
            self.inner.delete(ctx, id, removal).await
        }
        async fn list(
            &self,
            ctx: &SecurityContext,
            query: &Query<GroupField>,
        ) -> StoreResult<Page<Group>> {
            self.inner.list(ctx, query).await
        }
        async fn restore(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
            self.fails()?;
            self.inner.restore(ctx, id).await
        }
        async fn begin_purge(
            &self,
            ctx: &SecurityContext,
            id: GroupId,
        ) -> StoreResult<Option<Group>> {
            self.inner.begin_purge(ctx, id).await
        }
        async fn purge(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool> {
            self.inner.purge(ctx, id).await
        }
        async fn get_deleted(
            &self,
            ctx: &SecurityContext,
            id: GroupId,
        ) -> StoreResult<Deleted<Group>> {
            self.inner.get_deleted(ctx, id).await
        }
        async fn list_deleted(
            &self,
            ctx: &SecurityContext,
            query: &Query<GroupField>,
        ) -> StoreResult<Page<Deleted<Group>>> {
            self.inner.list_deleted(ctx, query).await
        }
        async fn list_deleted_with(
            &self,
            ctx: &SecurityContext,
            removal_id: RemovalId,
            query: &Query<GroupField>,
        ) -> StoreResult<Page<Group>> {
            self.inner.list_deleted_with(ctx, removal_id, query).await
        }
        async fn find_by_name(
            &self,
            ctx: &SecurityContext,
            name: &ScopedName,
        ) -> StoreResult<Group> {
            self.inner.find_by_name(ctx, name).await
        }
    }

    #[tokio::test]
    async fn the_participants_follow_again_once_their_group_was_left_deleted() {
        let groups = Persistence::in_memory().entity_stores().unwrap().groups;
        let system = SecurityContext::system();
        let owner = UserId::new();
        let bob = ActorId::User(UserId::new());
        let session = SessionId::new();
        let name =
            ScopedName::new("participants", Scope::SessionScope { session_id: session }).unwrap();
        let participants = Group { id: GroupId::of_session(session), ..Group::team(name, owner) };
        keep_participants(groups.as_ref(), participants.clone()).await.expect("made");
        groups.delete(&system, participants.id, &Removal::now(system.caller_id)).await.unwrap();

        keep_participants(groups.as_ref(), Group { members: vec![bob], ..participants.clone() })
            .await
            .expect("restored, not made again");
        assert_eq!(
            groups.get(&system, participants.id).await.unwrap().members,
            [bob],
            "there again, following who takes part"
        );
    }

    /// Issue 492: the members are set once more, on the group as it is now, when the group was
    /// written meanwhile.
    #[tokio::test]
    async fn the_participants_follow_once_more_when_their_group_was_written_meanwhile() {
        let (stores, refusals) = ContendedStore::stores(Group::KIND);
        let groups = stores.entity_stores().unwrap().groups;
        let system = SecurityContext::system();
        let bob = ActorId::User(UserId::new());
        let session = SessionId::new();
        let name =
            ScopedName::new("participants", Scope::SessionScope { session_id: session }).unwrap();
        let participants =
            Group { id: GroupId::of_session(session), ..Group::team(name, UserId::new()) };
        keep_participants(groups.as_ref(), participants.clone()).await.expect("made");

        refusals.store(1, Ordering::SeqCst);
        keep_participants(groups.as_ref(), Group { members: vec![bob], ..participants.clone() })
            .await
            .expect("set once more");
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "refused once");
        assert_eq!(groups.get(&system, participants.id).await.unwrap().members, [bob]);
    }

    /// Issue 492: a team is changed once more, as it is now, when it was written meanwhile.
    #[tokio::test]
    async fn a_team_is_changed_once_more_when_it_was_written_meanwhile() {
        let (stores, refusals) = ContendedStore::stores(Group::KIND);
        let stores = stores.entity_stores().unwrap();
        let events = Arc::new(EventBus::new(
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        ));
        let manager = GroupManager::new(&stores, events, Arc::new(AllowAllPolicy::new()));
        let alice = UserId::new();
        let hers =
            SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        let team =
            manager.create(&hers, "Helpers", Scope::UserScope { user_id: alice }).await.unwrap();

        refusals.store(1, Ordering::SeqCst);
        let renamed = manager.rename(&hers, team.id, "Experts").await.expect("changed once more");
        assert_eq!(renamed.name.name, "Experts");
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "refused once");
        assert_eq!(stores.groups.get(&hers, team.id).await.unwrap().name.name, "Experts");
    }

    /// Issue 348: the groups of the participants of a session and its channel, and of the members
    /// of its own project (issue 467), go and come back with it all or none - one that fails takes
    /// back those done before it -, so that a session still there, or left deleted, has them as it
    /// has itself.
    #[tokio::test]
    async fn the_participants_go_and_come_back_all_or_none() {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let failing = Arc::new(Failing {
            inner: Arc::clone(&stores.groups),
            passing: AtomicUsize::new(usize::MAX),
        });
        let events = Arc::new(EventBus::new(
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        ));
        let manager = GroupManager::new(
            &EntityStores { groups: Arc::<Failing>::clone(&failing), ..stores.clone() },
            events,
            Arc::new(AllowAllPolicy::new()),
        );
        let system = SecurityContext::system();
        let owner = UserId::new();
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        let participants = |id: GroupId, scope: Scope| Group {
            id,
            ..Group::team(ScopedName::new("participants", scope).unwrap(), owner)
        };
        let of_session =
            participants(GroupId::of_session(session_id), Scope::SessionScope { session_id });
        let of_channel = participants(
            GroupId::of_channel(channel_id),
            Scope::ChannelScope { session_id, channel_id },
        );
        let own = Project::own_of(session_id, owner);
        let of_project = Group::of_project(&own);
        let groups = [&of_session, &of_channel, &of_project];
        for group in groups {
            keep_participants(stores.groups.as_ref(), group.clone()).await.unwrap();
        }
        let holders = [Scope::SessionScope { session_id }, own.holds()];
        let there = async |id: GroupId| stores.groups.get(&system, id).await.is_ok();
        let how_many_there = async || {
            let mut count = 0;
            for group in groups {
                count += usize::from(there(group.id).await);
            }
            count
        };

        let removal = Removal::now(ActorId::User(owner));
        failing.passing.store(2, Ordering::SeqCst);
        manager.delete_participants(&holders, &removal).await.expect_err("the third fails");
        assert_eq!(how_many_there().await, 3, "none is deleted: those before are restored again");

        failing.passing.store(usize::MAX, Ordering::SeqCst);
        manager.delete_participants(&holders, &removal).await.expect("all deleted");
        assert_eq!(how_many_there().await, 0);
        let marked = stores.groups.get_deleted(&system, of_session.id).await.unwrap().removal;
        assert_eq!(marked, removal, "with the mark of what holds them, in the runtime's name");

        failing.passing.store(2, Ordering::SeqCst);
        manager.restore_participants(&holders, &removal).await.expect_err("the third fails");
        assert_eq!(how_many_there().await, 0, "none is restored: those before are deleted again");

        failing.passing.store(usize::MAX, Ordering::SeqCst);
        manager.restore_participants(&holders, &removal).await.expect("all restored");
        assert_eq!(how_many_there().await, 3);
    }

    #[tokio::test]
    async fn only_the_entries_naming_a_group_known_to_be_gone_are_dropped() {
        let memory = Arc::new(MemoryStore::new());
        let groups = Persistence::over(
            Arc::<MemoryStore>::clone(&memory),
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .unwrap()
        .entity_stores()
        .unwrap()
        .groups;
        let system = SecurityContext::system();
        let owner = UserId::new();
        let team = |name: &str| {
            Group::team(ScopedName::new(name, Scope::UserScope { user_id: owner }).unwrap(), owner)
        };
        let there = team("there");
        let deleted = team("deleted");
        let purged = team("purged");
        let unreadable = team("unreadable");
        for group in [&there, &deleted, &purged, &unreadable] {
            groups.create(&system, group).await.unwrap();
        }
        groups.delete(&system, deleted.id, &Removal::now(system.caller_id)).await.unwrap();
        groups.delete(&system, purged.id, &Removal::now(system.caller_id)).await.unwrap();
        groups.purge(&system, purged.id).await.unwrap();
        // Kept as something that is no group, past the store.
        let storage = memory
            .documents(Group::KIND, |_| Err(StoreError::Backend("not read".to_string())))
            .unwrap();
        storage
            .replace(
                Document {
                    id: unreadable.id.into(),
                    revision: unreadable.revision.next(),
                    name: None,
                    removal_id: None,
                    security_attributes: SecurityAttributes::systems(),
                    body: Value::from("not a group"),
                },
                unreadable.revision,
            )
            .await
            .unwrap();

        let entry =
            |group: &Group| AclEntry { subject: Subject::Group(group.id), grant: Grant::Read };
        let everyone = AclEntry { subject: Subject::Everyone, grant: Grant::Read };
        let mut acl =
            vec![entry(&there), entry(&deleted), entry(&purged), entry(&unreadable), everyone];
        let mut of_version = vec![entry(&purged)];
        prune_gone_groups(groups.as_ref(), [&mut acl, &mut of_version]).await;
        assert_eq!(acl, [entry(&there), entry(&deleted), entry(&unreadable), everyone], "the one purged alone: deleted, it may come back, and one that cannot be read may be there");
        assert!(of_version.is_empty(), "from every list given");
    }
}
