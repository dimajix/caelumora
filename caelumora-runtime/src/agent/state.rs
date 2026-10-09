//! One owner of an agent's `AgentState` per process (issue 62, step 3).
//!
//! An agent takes part in several channels, and agents of user or global
//! scope in several sessions, each with an `AgentActor` of its own. Their
//! `AgentState` is the agent's, not a channel's, so it must not be a copy per
//! actor - the last one to store its copy would overwrite the others'
//! changes. The `RuntimeManager` keeps one [`AgentStateHandle`] per agent in
//! [`AgentStates`]; every actor of the agent holds the same handle. A turn
//! reads the state as it is (`AgentStateHandle::current`, a snapshot that
//! does not change under it) and stages its changes; when it completes, they
//! are applied to the state as it is then, under the handle's lock, and
//! stored - so no turn overwrites another's changes, and every next turn, in
//! whatever channel, sees them.

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use caelumora_model::agent::{AgentId, AgentState};
use caelumora_model::memory::{MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::message::MessageId;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityAttributes;
use caelumora_model::store::StateStore;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use tracing::debug;

/// The state of one agent, shared by all its actors.
#[derive(Debug)]
pub struct AgentStateHandle {
    agent_id: AgentId,
    current: ArcSwap<AgentState>,
    /// Held while changes are applied and stored, so that they are applied
    /// one after another to the state as it is.
    write: Mutex<()>,
    /// The number the next memory entry gets. Handed out when a turn
    /// remembers something, not when the turn completes: turns in several
    /// channels may remember at the same time, and each needs its number at
    /// once to tell the model.
    next_memory_id: AtomicU32,
    state_store: Arc<dyn StateStore>,
    events: Arc<dyn EventPublisher>,
    /// The agent's attributes: whoever may read the agent hears of changes
    /// to its state.
    attributes: SecurityAttributes,
}

impl AgentStateHandle {
    fn new(agent_id: AgentId, state: AgentState, state_store: Arc<dyn StateStore>, events: Arc<dyn EventPublisher>, attributes: SecurityAttributes) -> Self {
        let next_memory_id = AtomicU32::new(state.memory.next_id().0);
        Self { agent_id, current: ArcSwap::from_pointee(state), write: Mutex::new(()), next_memory_id, state_store, events, attributes }
    }

    /// A handle of an empty state, stored in a store of its own and
    /// announced to nobody - for turns in tests.
    #[cfg(test)]
    pub(crate) fn detached() -> Arc<Self> {
        let state_store = Arc::new(caelumora_model::store::memory::MemoryStore::new());
        Arc::new(Self::new(AgentId::default(), AgentState::default(), state_store, Arc::new(()), SecurityAttributes::default()))
    }

    /// The state as it is now. A snapshot: changes applied afterwards do not
    /// change it.
    pub fn current(&self) -> Arc<AgentState> {
        self.current.load_full()
    }

    /// A number for a new memory entry, never handed out before.
    pub fn reserve_memory_id(&self) -> MemoryId {
        MemoryId(self.next_memory_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Applies `changes` to the memory, each made at its origin in a turn
    /// that ended with message `after`, to the state as it is, stores it,
    /// makes it the current state, and announces it as
    /// `GlobalEvent::AgentStateChanged`. Nothing is changed if storing
    /// fails.
    ///
    /// # Errors
    /// Whatever the `StateStore` reports.
    pub async fn apply(&self, changes: Vec<(MemoryChange, MemoryOrigin)>, after: MessageId) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let _writing = self.write.lock().await;
        let mut state = (*self.current.load_full()).clone();
        let count = changes.len();
        for (change, origin) in changes {
            state.memory.push(change, origin, after);
        }
        self.state_store.put_agent_state(self.agent_id, &mut state).await
            .with_context(|| format!("Cannot store the state of agent {}", self.agent_id))?;
        self.current.store(Arc::new(state));
        self.events.publish(GlobalEvent::AgentStateChanged(self.agent_id), self.attributes.clone());
        debug!("Applied {} changes to the state of agent {}", count, self.agent_id);
        Ok(())
    }
}

/// The [`AgentStateHandle`]s of the process, one per agent while any of its
/// actors runs.
#[derive(Debug)]
pub struct AgentStates {
    handles: Mutex<HashMap<AgentId, Weak<AgentStateHandle>>>,
    state_store: Arc<dyn StateStore>,
    events: Arc<dyn EventPublisher>,
}

impl AgentStates {
    pub fn new(state_store: Arc<dyn StateStore>, events: Arc<dyn EventPublisher>) -> Self {
        Self { handles: Mutex::new(HashMap::new()), state_store, events }
    }

    /// The handle of `agent_id`: the one its other actors hold, or loaded
    /// from the `StateStore` if none does. Loaded once, however many actors
    /// ask at the same time. `attributes` are the agent's: whoever may read
    /// the agent hears of changes to its state.
    ///
    /// # Errors
    /// Whatever the `StateStore` reports when loading.
    pub async fn get(&self, agent_id: AgentId, attributes: SecurityAttributes) -> Result<Arc<AgentStateHandle>> {
        let mut handles = self.handles.lock().await;
        if let Some(handle) = handles.get(&agent_id).and_then(Weak::upgrade) {
            return Ok(handle);
        }
        let state = self.state_store.get_agent_state(agent_id).await
            .with_context(|| format!("Cannot load the state of agent {}", agent_id))?;
        let handle = Arc::new(AgentStateHandle::new(agent_id, state, Arc::clone(&self.state_store), Arc::clone(&self.events), attributes));
        handles.retain(|_, h| h.strong_count() > 0);
        handles.insert(agent_id, Arc::downgrade(&handle));
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::store::memory::MemoryStore;

    fn remember(id: MemoryId, text: &str) -> (MemoryChange, MemoryOrigin) {
        (MemoryChange::Add { id, text: text.to_string(), classification: Classification::CHANNEL_DEFAULT }, MemoryOrigin::default())
    }

    #[tokio::test]
    async fn every_actor_of_an_agent_shares_one_state_and_no_change_is_lost() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        let states = AgentStates::new(Arc::clone(&store), Arc::new(()));
        let agent = AgentId::new();
        let (in_main, in_research) = (states.get(agent, Default::default()).await.unwrap(), states.get(agent, Default::default()).await.unwrap());
        assert!(Arc::ptr_eq(&in_main, &in_research), "one handle for both channels");

        // Two turns, each from a snapshot taken before the other's changes.
        let before = in_main.current();
        let (a, b) = (in_main.reserve_memory_id(), in_research.reserve_memory_id());
        assert_ne!(a, b, "numbers are handed out once");
        in_main.apply(vec![remember(a, "from main")], MessageId::new()).await.unwrap();
        in_research.apply(vec![remember(b, "from research")], MessageId::new()).await.unwrap();

        let texts: Vec<_> = in_main.current().memory.current().into_iter().map(|m| m.text).collect();
        assert_eq!(texts, ["from main", "from research"], "neither overwrote the other");
        assert!(before.memory.current().is_empty(), "a snapshot does not change under its reader");
        let stored = store.get_agent_state(agent).await.unwrap();
        assert_eq!(stored.memory.current().len(), 2, "stored as applied");
    }

    #[tokio::test]
    async fn a_released_state_is_loaded_again_with_what_was_stored() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        let states = AgentStates::new(store, Arc::new(()));
        let agent = AgentId::new();
        let handle = states.get(agent, Default::default()).await.unwrap();
        let id = handle.reserve_memory_id();
        handle.apply(vec![remember(id, "kept")], MessageId::new()).await.unwrap();
        drop(handle);

        let again = states.get(agent, Default::default()).await.unwrap();
        assert_eq!(again.current().memory.current()[0].text, "kept");
        assert_eq!(again.reserve_memory_id(), MemoryId(id.0 + 1), "numbering continues after what was stored");
    }

    #[derive(Debug, Default)]
    struct Announced(std::sync::Mutex<Vec<GlobalEvent>>);

    impl EventPublisher for Announced {
        fn publish(&self, event: GlobalEvent, _attributes: SecurityAttributes) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn a_stored_change_is_announced_without_its_content() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        let announced = Arc::new(Announced::default());
        let states = AgentStates::new(store, Arc::<Announced>::clone(&announced));
        let agent = AgentId::new();
        let handle = states.get(agent, Default::default()).await.unwrap();

        handle.apply(Vec::new(), MessageId::new()).await.unwrap();
        assert!(announced.0.lock().unwrap().is_empty(), "nothing changed, nothing announced");
        let id = handle.reserve_memory_id();
        handle.apply(vec![remember(id, "secret")], MessageId::new()).await.unwrap();
        let events = announced.0.lock().unwrap();
        assert!(matches!(events.as_slice(), [GlobalEvent::AgentStateChanged(a)] if *a == agent), "{events:?}");
    }
}
