use crate::agent::{AgentChannelState, AgentId, AgentState};
use crate::chain::{Chain, Node, Stored as StoredChain};
use crate::channel::{ChannelId, ChannelState};
use crate::history::{History, HistoryEntry, HistoryPage};
use crate::message::{Message, MessageId};
use crate::component::StoredComponents;
use crate::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use crate::removal::{Deletable, Removal};
use crate::security::{ActionType, AllowAllPolicy, SecurityContext, AccessPolicy};
use crate::store::access::{name_free, Access};
use crate::store::history::{page_of, HistoryKey, HistoryStore, StandingItems};
use crate::store::query::{self, Filter, NoField, Page, Query};
use crate::store::{ComponentStore, EntityStore, EntityStoreFactory, EntityStores, Journaled, Policies, StateStore, StoreError, StoreResult, Stored};
use crate::type_map::TypeMap;
use crate::value::name::ScopedName;
use async_trait::async_trait;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::{self, Debug};
use std::future::ready;
use std::sync::{Arc, RwLock};
use tracing::info;

/// Every kind's entities, the preconfigured components and the runtime
/// state, in memory: a store per kind (`MemoryEntityStore`, issue 323),
/// opened when it is first asked for, and maps for the states - their
/// documents, and their histories apart, each whole with its items as they
/// stand (issue 379).
#[derive(Debug)]
pub struct MemoryStore {
    policies: Policies,
    /// The store of every kind asked for, by its type.
    kinds: RwLock<TypeMap>,
    components: MemoryEntityStore<StoredComponents>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    channel_states: RwLock<HashMap<ChannelId, ChannelState>>,
    histories: RwLock<HashMap<HistoryKey, KeptHistory>>,
}

/// A history as the memory store keeps it: its chain whole, and the items
/// derived from it.
#[derive(Debug, Default)]
struct KeptHistory {
    chain: History,
    items: StandingItems,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_policies(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()))
    }
}

impl MemoryStore {
    /// A store that permits everything - for tests and throwaway setups.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that asks `access_policy` about every access to an
    /// entity, and `confidentiality` whether the caller is cleared for what
    /// is reached by clearance.
    pub fn with_policies(access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> Self {
        info!("Creating memory store");
        let policies = (access_policy, confidentiality);
        Self {
            components: MemoryEntityStore::new(&policies),
            policies,
            kinds: Default::default(),
            agent_states: Default::default(),
            agent_channel_states: Default::default(),
            channel_states: Default::default(),
            histories: Default::default(),
        }
    }

    /// Appends what `history`, the history `key` names as a state holds it,
    /// added since it was last stored, derives the items of it, and marks it
    /// stored.
    fn append_history(&self, key: HistoryKey, history: &mut History) -> StoreResult<()> {
        let Some(changes) = history.changes() else { return Ok(()) };
        write_lock(&self.histories, |all| {
            let kept = all.entry(key).or_default();
            let end = kept.chain.end();
            kept.chain.apply(changes).map_err(|e| StoreError::Backend(format!("what {key} added does not fit: {e}")))?;
            for node in kept.chain.from(end) {
                kept.items.take(node);
            }
            Ok::<_, StoreError>(())
        })?;
        history.mark_stored();
        Ok(())
    }

    /// The history `key` names from the position `from` on, as a holder
    /// keeps it; empty if none is kept.
    fn load_history(&self, key: HistoryKey, from: u64) -> StoreResult<History> {
        read_lock(&self.histories, |all| {
            let Some(kept) = all.get(&key) else { return Ok(History::default()) };
            Chain::suffix(from, StoredChain { entries: kept.chain.from(from).cloned().collect(), head: kept.chain.digest() })
                .map_err(|e| StoreError::Backend(format!("{key} cannot be read from position {from}: {e}")))
        })
    }

    /// `state`'s document - what is stored of it but its history - as a
    /// copy without the history, which is kept apart.
    fn document<T: Clone>(state: &mut T, history: impl Fn(&mut T) -> &mut History) -> T {
        let held = std::mem::take(history(state));
        let document = state.clone();
        *history(state) = held;
        document
    }

    /// The stores of the core's entities, one per kind (issue 323).
    pub fn entity_stores(&self) -> EntityStores {
        match EntityStores::open(self) {
            Ok(stores) => stores,
            Err(never) => match never {},
        }
    }
}

/// Opens a kind in memory, which cannot fail.
impl EntityStoreFactory for MemoryStore {
    type Error = Infallible;

    fn entity_store<T: Deletable>(&self) -> Result<Arc<dyn EntityStore<T>>, Infallible> {
        write_lock(&self.kinds, |kinds| kinds.get_or_try_insert(|| Ok(Arc::new(MemoryEntityStore::<T>::new(&self.policies)) as Arc<dyn EntityStore<T>>)))
    }
}

// A poisoned lock is used anyway: the locks are held only by the closures
// passed here, none of which can leave a map half-changed.
fn read_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&T) -> R,
{
    match lock.read() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(poisoned.get_ref()),
    }
}

fn write_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    match lock.write() {
        Ok(mut guard) => f(&mut guard),
        Err(mut poisoned) => f(poisoned.get_mut()),
    }
}

/// The entities of one kind in memory (issue 323), checked as every kind is
/// (`store::access`).
pub struct MemoryEntityStore<T: Stored> {
    access_policy: Arc<dyn AccessPolicy>,
    /// Who is cleared for what is reached by clearance (issue 322).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    entities: RwLock<HashMap<T::Id, T>>,
}

impl<T: Stored> Debug for MemoryEntityStore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryEntityStore").field("kind", &T::KIND).finish_non_exhaustive()
    }
}

impl<T: Stored> MemoryEntityStore<T> {
    /// An empty store that asks the `AccessPolicy` and the
    /// `ConfidentialityPolicy` of `policies`.
    fn new(policies: &Policies) -> Self {
        Self { access_policy: Arc::clone(&policies.0), confidentiality: Arc::clone(&policies.1), entities: Default::default() }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.access_policy.as_ref(), self.confidentiality.as_ref(), ctx)
    }

    fn stored(&self, id: &T::Id) -> Option<T> {
        read_lock(&self.entities, |v| v.get(id).cloned())
    }

    /// Stores `value` as `update` and `restore` do: the others of its kind
    /// checked for its name and it written, in one step.
    fn write(&self, value: &T) -> StoreResult<()> {
        let id = value.id();
        write_lock(&self.entities, |v| {
            name_free(value, v.iter().filter(|(other, _)| **other != id).map(|(_, other)| other))?;
            v.insert(id, value.clone());
            Ok(())
        })
    }

    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        self.access(ctx).create(value).await?;
        let id = value.id();
        write_lock(&self.entities, |v| {
            if v.contains_key(&id) {
                return Err(StoreError::Conflict(format!("{} {} already exists", T::KIND, id)));
            }
            name_free(value, v.values())?;
            v.insert(id, value.clone());
            Ok(())
        })
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        self.access(ctx).read(self.stored(&id), id.to_string()).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        let id = value.id();
        self.access(ctx).change(self.stored(&id).as_ref(), value, id.to_string()).await?;
        self.write(value)
    }

    /// Removes the entity for good: deleting a kind that is not marked.
    async fn remove(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        if !self.access(ctx).delete(self.stored(&id).as_ref()).await? {
            return Ok(false);
        }
        Ok(write_lock(&self.entities, |v| v.remove(&id).is_some()))
    }

    /// The entity of `id`, if it is there and meets `filter`.
    fn meeting(&self, id: &T::Id, filter: Option<&Filter<T::Field>>) -> StoreResult<Option<T>> {
        match self.stored(id) {
            Some(value) => query::meeting(value, filter),
            None => Ok(None),
        }
    }

    /// The page `query` asks for, among what is deleted if `deleted`.
    async fn page(&self, ctx: &SecurityContext, query: &Query<T::Field>, deleted: bool) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let keys = read_lock(&self.entities, |v| v.keys().map(|id| (id.to_string(), id.clone())).collect());
        let filter = query.filter.as_ref();
        access.page(keys, query, deleted, |id: T::Id| ready(self.meeting(&id, filter))).await
    }
}

#[async_trait]
impl<T: Deletable> EntityStore<T> for MemoryEntityStore<T> {
    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        MemoryEntityStore::create(self, ctx, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        MemoryEntityStore::get(self, ctx, id).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        MemoryEntityStore::update(self, ctx, value).await
    }

    /// Marks the entity deleted, by the caller and now (issue 164).
    async fn delete(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let stored = self.stored(&id);
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        write_lock(&self.entities, |v| v.insert(id, marked));
        Ok(true)
    }

    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, false).await
    }

    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let mut restored = self.access(ctx).restore(self.stored(&id), id.to_string()).await?;
        restored.set_deleted(None);
        self.write(&restored)?;
        Ok(restored)
    }

    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        if !self.access(ctx).purge(self.stored(&id).as_ref()).await? {
            return Ok(false);
        }
        // Only if it is still marked: restored while the purge was asked
        // about, it stays.
        Ok(write_lock(&self.entities, |v| {
            let marked = v.get(&id).is_some_and(|value| value.deleted().is_some());
            marked && v.remove(&id).is_some()
        }))
    }

    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        self.access(ctx).read_deleted(self.stored(&id), id.to_string()).await
    }

    async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, true).await
    }

    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let all = self.page(ctx, &Query::all(), false).await?.items;
        all.into_iter().find(|value| value.unique_name().as_ref() == Some(name))
            .ok_or_else(|| StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() })
    }
}

/// Preconfigured components, kept like the entities (issue 164).
#[async_trait]
impl ComponentStore for MemoryStore {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.create(ctx, value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.components.get(ctx, id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.update(ctx, value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.components.remove(ctx, id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.components.page(ctx, query, false).await
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        let mut state = read_lock(&self.agent_channel_states, |v| v.get(&(agent_id, channel_id)).cloned()).unwrap_or_default();
        state.history = self.load_history(HistoryKey::Agent(agent_id, channel_id), state.window_from)?;
        Ok(state)
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()> {
        self.append_history(HistoryKey::Agent(agent_id, channel_id), &mut state.history)?;
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        let document = Self::document(state, |s| &mut s.history);
        write_lock(&self.agent_channel_states, |v| v.insert((agent_id, channel_id), document));
        Ok(())
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        Ok(read_lock(&self.agent_states, |v| v.get(&agent_id).cloned()).unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        write_lock(&self.agent_states, |v| v.insert(agent_id, state.clone()));
        Ok(())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let mut state = read_lock(&self.channel_states, |v| v.get(&channel_id).cloned()).ok_or_else(|| StoreError::NotFound { entity: "channel_state", id: channel_id.to_string() })?;
        state.history = self.load_history(HistoryKey::Channel(channel_id), state.window_from)?;
        Ok(state)
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &mut ChannelState) -> StoreResult<()> {
        self.append_history(HistoryKey::Channel(channel_id), &mut state.history)?;
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        let document = Self::document(state, |s| &mut s.history);
        write_lock(&self.channel_states, |v| v.insert(channel_id, document));
        Ok(())
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        write_lock(&self.channel_states, |v| v.remove(&channel_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(_, cid), _| *cid != channel_id));
        write_lock(&self.histories, |v| v.retain(|key, _| !key.in_channel(channel_id)));
        Ok(())
    }

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        write_lock(&self.agent_states, |v| v.remove(&agent_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(aid, _), _| *aid != agent_id));
        write_lock(&self.histories, |v| v.retain(|key, _| !key.of_agent(agent_id)));
        Ok(())
    }

}

#[async_trait]
impl HistoryStore for MemoryStore {
    async fn page(&self, key: HistoryKey, from: u64, before: Option<u64>, limit: usize) -> StoreResult<HistoryPage> {
        Ok(read_lock(&self.histories, |all| match all.get(&key) {
            Some(kept) => page_of(kept.items.newest_before(before.unwrap_or(u64::MAX)).cloned(), from, limit),
            None => HistoryPage::default(),
        }))
    }

    async fn message(&self, key: HistoryKey, id: MessageId) -> StoreResult<Option<(u64, Message)>> {
        Ok(read_lock(&self.histories, |all| all.get(&key).and_then(|kept| kept.items.message(id))))
    }

    async fn entries(&self, key: HistoryKey, from: u64) -> StoreResult<Vec<Node<HistoryEntry>>> {
        Ok(read_lock(&self.histories, |all| all.get(&key).map(|kept| kept.chain.from(from).cloned().collect()).unwrap_or_default()))
    }

    async fn rebuild(&self, key: HistoryKey) -> StoreResult<()> {
        write_lock(&self.histories, |all| {
            if let Some(kept) = all.get_mut(&key) {
                kept.items = StandingItems::of(kept.chain.all_entries());
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::suite;
    use crate::user::UserId;
    use crate::confidentiality::ConfidentialityRules;
    use crate::security::OwnershipPolicy;

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&Arc::new(MemoryStore::new()).entity_stores()).await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let store = Arc::new(MemoryStore::new());
        suite::queries_behave(&store.entity_stores()).await;
        let Ok(folders) = store.entity_store::<suite::Folder>();
        suite::queries_reach_parts(folders.as_ref()).await;
    }

    /// A limit counts what the caller may read (issue 293): the policy is
    /// asked as the list is read, and a page is filled from further on.
    #[tokio::test]
    async fn a_limit_counts_what_the_caller_may_read() {
        use crate::actor::ActorId;
        use crate::confidentiality::Classification;
        use crate::scope::Scope;
        use crate::security::OwnershipPolicy;
        use crate::workspace::Workspace;
        let store = Arc::new(MemoryStore::with_policies(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()))).entity_stores().workspaces;
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut hers = Vec::new();
        for (index, owner) in [alice, bob, bob, alice, bob, alice].into_iter().enumerate() {
            let name = ScopedName::new(format!("w{index}"), Scope::UserScope { user_id: owner }).unwrap();
            let workspace = Workspace::new(name, "main", Classification::CHANNEL_DEFAULT, owner);
            store.create(&SecurityContext::system(), &workspace).await.unwrap();
            if owner == alice {
                hers.push(workspace.id);
            }
        }
        hers.sort_by_key(|id| id.to_string());
        let ctx = SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        let mut read = Vec::new();
        let mut query = Query::all().limited(1);
        loop {
            let page = store.list(&ctx, &query).await.unwrap();
            read.extend(page.items.iter().map(|w| w.id));
            match page.next {
                Some(cursor) => {
                    assert_eq!(page.items.len(), 1, "every page that goes on is full, however many of bob's lie between");
                    query = Query::all().limited(1).after(cursor);
                }
                // The last page may be empty: what followed was bob's.
                None => break,
            }
        }
        assert_eq!(read, hers, "hers alone, in the order of their ids");
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        suite::state_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_the_histories_of_the_states() {
        let store = MemoryStore::new();
        suite::history_store_behaves(&store, &store).await;
    }

    #[tokio::test]
    async fn behaves_as_a_component_store() {
        suite::component_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        suite::deleting_marks(&Arc::new(MemoryStore::new()).entity_stores()).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        suite::names_are_unique(&Arc::new(MemoryStore::new()).entity_stores()).await;
        let owners = MemoryStore::with_policies(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        suite::names_are_unique_beyond_what_is_read(&Arc::new(owners).entity_stores()).await;
    }
}
