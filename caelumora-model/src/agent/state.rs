use crate::chain::Stored;
use crate::ledger::LedgerEntry;
use crate::memory::{Memories, MemoryChain};
use crate::store::Journaled;
use serde::{Deserialize, Serialize};

/// The agent's own state changed - its memory (issue 62), announced as
/// `EntityChange::Custom` (issue 323) with the agent's attributes once the
/// change is stored; without content, since the state is private: whoever
/// may see it fetches it.
pub const STATE_CHANGED: &str = "state_changed";

/// The state of an agent itself, across its channels and - for agents of
/// user or global scope - across sessions (issue 62). One owner per agent
/// and process holds it; turns read a snapshot and stage their changes
/// (`TurnState::memory`), which are applied to the current state when the
/// turn completes, so that no turn overwrites another's changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    /// What the agent remembers across its channels and sessions.
    pub memory: MemoryChain,
}

/// A change to an [`AgentState`] since it was last stored (issue 193).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStateChange {
    Memory(Stored<LedgerEntry<Memories>>),
}

impl Journaled for AgentState {
    type Change = AgentStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        self.memory.changes().map(AgentStateChange::Memory).into_iter().collect()
    }

    fn mark_stored(&mut self) {
        self.memory.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            AgentStateChange::Memory(change) => self.memory.apply(change),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Classification;
    use crate::message::MessageId;
    use crate::store::testing::{replayed, stored};

    #[test]
    fn what_changed_in_an_agents_own_state_replays_onto_what_was_stored() {
        use crate::memory::{MemoryChange, MemoryId};
        let mut state = AgentState::default();
        state.memory.push(
            MemoryChange::Add {
                id: MemoryId(1),
                text: "a".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            Default::default(),
            MessageId::default(),
        );
        let mut copy = stored(&mut state);
        assert!(state.changes().is_empty(), "nothing between turns");
        state.memory.push(
            MemoryChange::Remove { id: MemoryId(1) },
            Default::default(),
            MessageId::default(),
        );
        assert!(matches!(state.changes().as_slice(), [AgentStateChange::Memory(_)]));
        replayed(&state, &mut copy);
        assert!(copy.memory.current().is_empty());
    }
}
