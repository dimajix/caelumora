//! What an agent remembers across its channels and sessions (issue 62),
//! read and changed within a turn.
//!
//! The memory is part of the agent's `AgentState`; `Turn` offers only
//! generic access to that state - a snapshot, staged changes - and knows
//! nothing of memory. [`AgentMemory`] is the logic on top: which entries may
//! be recalled in the turn's channel, remembering and forgetting. The
//! `agent_memory` component uses it for its tools and prompt part; anything
//! else that works with an agent's memory in a turn uses it too.

use crate::agent_turn::Turn;
use caelumora_model::agent::AgentStateChange;
use caelumora_model::digest::Digest;
use caelumora_model::memory::{Memory, MemoryChange, MemoryId};

/// An agent's memory as a turn sees it: every function takes the turn.
pub struct AgentMemory;

impl AgentMemory {
    /// What may be recalled in `turn`'s channel: entries written in a
    /// channel it may read (`ChannelClassification::readable_from`, rule 2
    /// of issue 61), as the memory is now - without the turn's own changes.
    pub fn recalled(turn: &Turn) -> Vec<Memory> {
        let here = turn.classification();
        turn.agent_state().current().memory.current().into_iter()
            .filter(|m| m.origin.readable_from(&here))
            .collect()
    }

    /// The digest of the memory as it is now: the state of knowledge a turn
    /// that recalls from it records.
    pub fn digest(turn: &Turn) -> Option<Digest> {
        turn.agent_state().current().memory.digest()
    }

    /// Remembers `text`, as written in the turn's channel - taken over when
    /// the turn completes. Returns its number.
    ///
    /// Unlike a note's, the number is reserved on the agent's shared state
    /// handle (`AgentStateHandle::reserve_memory_id`) instead of being
    /// derived from the memory and the turn's staged changes: several actors
    /// of one agent - in other channels and sessions - remember at the same
    /// time, and would otherwise hand out the same number. Deliberately the
    /// one exception to "commit, don't write" (issue 19).
    pub fn remember(turn: &mut Turn, text: String) -> MemoryId {
        let origin = turn.classification();
        let id = turn.agent_state().reserve_memory_id();
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Remember { id, text }, origin });
        id
    }

    /// Forgets entry `id` if it may be recalled here or was remembered
    /// earlier in this turn, and is not forgotten already - taken over when
    /// the turn completes. Returns whether it was.
    pub fn forget(turn: &mut Turn, id: MemoryId) -> bool {
        let origin = turn.classification();
        let recalled = Self::recalled(turn).iter().any(|m| m.id == id);
        let staged = |wanted: fn(&MemoryChange, MemoryId) -> bool| turn.staged_agent_changes().iter()
            .any(|c| matches!(c, AgentStateChange::Memory { change, .. } if wanted(change, id)));
        let remembered_now = staged(|c, id| matches!(c, MemoryChange::Remember { id: r, .. } if *r == id));
        let forgotten = staged(|c, id| matches!(c, MemoryChange::Forget { id: f } if *f == id));
        if !(recalled || remembered_now) || forgotten {
            return false;
        }
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Forget { id }, origin });
        true
    }
}
