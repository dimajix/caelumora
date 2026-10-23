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
//! failed is corrected by the next. The group goes and comes back with its
//! session or channel, not with what lies in its scope (issue 348): what
//! deletes, restores and purges the session or channel deletes, restores
//! and purges the group with it, in the runtime's name as well - the
//! `GroupManager` deletes and restores it, announcing it as a team
//! (`…_participants`) -, so that a session or channel that is there has
//! its group there, too.
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

/// Whether `ctx` may name every group among `entries` that `before` does
/// not name already: it reads the group (`GetGroup`, asked by the store).
/// What a list named before stays, so that its owner keeps a list they
/// change, whoever named a group in it.
///
/// # Errors
/// `StoreError::Validation` naming the first group `ctx` may not read, or
/// whatever else the store reports.
pub async fn require_nameable(store: &dyn EntityStore<Group>, ctx: &SecurityContext, before: &[AclEntry], entries: &[AclEntry]) -> StoreResult<()> {
    for entry in entries {
        let Subject::Group(group) = entry.subject else { continue };
        if before.iter().any(|named| named.subject == entry.subject) {
            continue;
        }
        if store.get(ctx, group).await.optional()?.is_none() {
            return Err(StoreError::Validation(format!("an access list names group {group}, which you cannot read")));
        }
    }
    Ok(())
}

/// Drops from `lists` - the access lists of one entity - the entries that
/// name a group which is gone (issue 347): purged, neither there nor in the
/// trash, as the runtime finds it. Called wherever such an entity is
/// stored, so that an entry for a session long purged goes as the list is
/// next written, not as the group is purged: that would have to find every
/// list naming it, of every kind. A group in the trash comes back with
/// what it belongs to and keeps its entries, and so does one that cannot be
/// looked up: an entry is dropped only for certain.
pub async fn prune_gone_groups<'a>(store: &dyn EntityStore<Group>, lists: impl IntoIterator<Item = &'a mut Vec<AclEntry>>) {
    let mut lists: Vec<&mut Vec<AclEntry>> = lists.into_iter().collect();
    let mut named = Vec::new();
    for entry in lists.iter().flat_map(|list| list.iter()) {
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
    if gone.is_empty() {
        return;
    }
    for list in &mut lists {
        list.retain(|entry| !matches!(entry.subject, Subject::Group(group) if gone.contains(&group)));
    }
    debug!(groups = ?gone, "Entries naming groups that are gone dropped from an access list");
}

/// Whether group `id` is neither there nor in the trash, looked for in the
/// runtime's name; `false` if that cannot be told.
async fn is_gone(store: &dyn EntityStore<Group>, id: GroupId) -> bool {
    let system = SecurityContext::system();
    let found = match store.get(&system, id).await.optional() {
        Ok(None) => store.get_deleted(&system, id).await.optional().map(|deleted| deleted.is_some()),
        there => there.map(|live| live.is_some()),
    };
    found.map(|found| !found).unwrap_or_else(|e| {
        warn!(group = %id, "Cannot tell whether group is gone; the entries naming it stay: {e}");
        false
    })
}

/// Stores `participants` as the group of the participants of a session or
/// channel, in the runtime's name: its members replace those stored, and
/// the group is made if there is none yet. Called only while the session
/// or channel is there, so a group found in the trash is restored first
/// (issue 347): it goes and comes back with the session or channel (issue
/// 348), but taking back a deletion that failed may have failed, too.
///
/// # Errors
/// Whatever the store reports.
pub(crate) async fn keep_participants(store: &dyn EntityStore<Group>, participants: Group) -> StoreResult<()> {
    let system = SecurityContext::system();
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
    stored.members = participants.members;
    stored.updated_at = Utc::now();
    store.update(&system, &stored).await
}

/// Removes the group `id` for good, in the runtime's name: marked deleted
/// first if it is not, since only what was deleted is purged (issue 164).
/// For the participants of a session or a channel purged, and of one whose
/// creation failed. `false` if there is no such group.
///
/// # Errors
/// Whatever else the store reports.
pub(crate) async fn purge_participants(store: &dyn EntityStore<Group>, id: GroupId) -> StoreResult<bool> {
    let system = SecurityContext::system();
    if store.delete(&system, id).await.optional()?.is_none() {
        return Ok(false);
    }
    store.purge(&system, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;
    use caelumora_model::actor::ActorId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::Grant;
    use caelumora_model::store::EntityStores;
    use caelumora_model::value::name::ScopedName;
    use std::sync::Arc;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_model::session::SessionId;
    use caelumora_model::store::documents::Document;
    use serde_json::Value;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::{PersistenceBackend, Stored};
    use caelumora_model::store::Persistence;
    use caelumora_model::store::query::{Page, Query};
    use caelumora_model::channel::ChannelId;
    use caelumora_model::group::GroupField;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        async fn update(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()> {
            self.inner.update(ctx, value).await
        }
        async fn delete(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool> {
            self.fails()?;
            self.inner.delete(ctx, id).await
        }
        async fn list(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>> {
            self.inner.list(ctx, query).await
        }
        async fn restore(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
            self.fails()?;
            self.inner.restore(ctx, id).await
        }
        async fn purge(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool> {
            self.inner.purge(ctx, id).await
        }
        async fn get_deleted(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
            self.inner.get_deleted(ctx, id).await
        }
        async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>> {
            self.inner.list_deleted(ctx, query).await
        }
        async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Group> {
            self.inner.find_by_name(ctx, name).await
        }
    }

    #[tokio::test]
    async fn the_participants_follow_again_once_their_group_was_left_in_the_trash() {
        let groups = Persistence::in_memory().entity_stores().unwrap().groups;
        let system = SecurityContext::system();
        let owner = UserId::new();
        let bob = ActorId::User(UserId::new());
        let session = SessionId::new();
        let name = ScopedName::new("participants", Scope::SessionScope { session_id: session }).unwrap();
        let participants = Group { id: GroupId::of_session(session), ..Group::team(name, owner) };
        keep_participants(groups.as_ref(), participants.clone()).await.expect("made");
        groups.delete(&system, participants.id).await.unwrap();

        keep_participants(groups.as_ref(), Group { members: vec![bob], ..participants.clone() }).await.expect("restored, not made again");
        assert_eq!(groups.get(&system, participants.id).await.unwrap().members, [bob], "there again, following who takes part");
    }

    /// Issue 348: the groups of the participants of a session and its
    /// channel go and come back with it all or none - one that fails takes
    /// back those done before it -, so that a session kept, or left in the
    /// trash, has them as it has itself.
    #[tokio::test]
    async fn the_participants_go_and_come_back_all_or_none() {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let failing = Arc::new(Failing { inner: Arc::clone(&stores.groups), passing: AtomicUsize::new(usize::MAX) });
        let events = Arc::new(EventBus::new(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())));
        let manager = GroupManager::new(&EntityStores { groups: Arc::<Failing>::clone(&failing), ..stores.clone() }, events, Arc::new(AllowAllPolicy::new()));
        let system = SecurityContext::system();
        let owner = UserId::new();
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        let participants = |id: GroupId, scope: Scope| Group { id, ..Group::team(ScopedName::new("participants", scope).unwrap(), owner) };
        let of_session = participants(GroupId::of_session(session_id), Scope::SessionScope { session_id });
        let of_channel = participants(GroupId::of_channel(channel_id), Scope::ChannelScope { session_id, channel_id });
        for group in [&of_session, &of_channel] {
            keep_participants(stores.groups.as_ref(), group.clone()).await.unwrap();
        }
        let holder = Scope::SessionScope { session_id };
        let there = async |id: GroupId| stores.groups.get(&system, id).await.is_ok();

        failing.passing.store(1, Ordering::SeqCst);
        manager.delete_participants(&holder).await.expect_err("the second fails");
        assert!(there(of_session.id).await && there(of_channel.id).await, "neither is deleted: the first is restored again");

        failing.passing.store(usize::MAX, Ordering::SeqCst);
        let since = Utc::now();
        manager.delete_participants(&holder).await.expect("both deleted");
        assert!(!there(of_session.id).await && !there(of_channel.id).await);

        failing.passing.store(1, Ordering::SeqCst);
        manager.restore_participants(&holder, since).await.expect_err("the second fails");
        assert!(!there(of_session.id).await && !there(of_channel.id).await, "neither is restored: the first is deleted again");

        failing.passing.store(usize::MAX, Ordering::SeqCst);
        manager.restore_participants(&holder, since).await.expect("both restored");
        assert!(there(of_session.id).await && there(of_channel.id).await);
    }

    #[tokio::test]
    async fn only_the_entries_naming_a_group_known_to_be_gone_are_dropped() {
        let memory = Arc::new(MemoryStore::new());
        let groups = Persistence::over(Arc::<MemoryStore>::clone(&memory), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap().entity_stores().unwrap().groups;
        let system = SecurityContext::system();
        let owner = UserId::new();
        let team = |name: &str| Group::team(ScopedName::new(name, Scope::UserScope { user_id: owner }).unwrap(), owner);
        let there = team("there");
        let trashed = team("trashed");
        let purged = team("purged");
        let unreadable = team("unreadable");
        for group in [&there, &trashed, &purged, &unreadable] {
            groups.create(&system, group).await.unwrap();
        }
        groups.delete(&system, trashed.id).await.unwrap();
        groups.delete(&system, purged.id).await.unwrap();
        groups.purge(&system, purged.id).await.unwrap();
        // Kept as something that is no group, past the store.
        let storage = memory.documents(Group::KIND, |_| Err(StoreError::Backend("not read".to_string()))).unwrap();
        storage.replace(Document { id: unreadable.id.into(), name: None, deleted: false, body: Value::from("not a group") }).await.unwrap();

        let entry = |group: &Group| AclEntry { subject: Subject::Group(group.id), grant: Grant::Read };
        let everyone = AclEntry { subject: Subject::Everyone, grant: Grant::Read };
        let mut acl = vec![entry(&there), entry(&trashed), entry(&purged), entry(&unreadable), everyone];
        let mut of_version = vec![entry(&purged)];
        prune_gone_groups(groups.as_ref(), [&mut acl, &mut of_version]).await;
        assert_eq!(acl, [entry(&there), entry(&trashed), entry(&unreadable), everyone], "the one purged alone: in the trash it may come back, and one that cannot be read may be there");
        assert!(of_version.is_empty(), "from every list given");
    }
}
