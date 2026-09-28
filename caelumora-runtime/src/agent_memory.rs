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

/// An agent's memory as a turn sees it.
pub struct AgentMemory<'t, 'a> {
    turn: &'t mut Turn<'a>,
}

impl<'t, 'a> AgentMemory<'t, 'a> {
    pub fn new(turn: &'t mut Turn<'a>) -> Self {
        Self { turn }
    }

    /// What may be recalled in `turn`'s channel: entries written in a
    /// channel it may read (`ChannelClassification::readable_from`, rule 2
    /// of issue 61), as the memory is now - without the turn's own changes.
    /// Nothing outside of a turn in a channel.
    pub fn recalled(turn: &Turn) -> Vec<Memory> {
        let Some(here) = turn.classification() else { return Vec::new() };
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
    /// the turn completes. Returns its number, or `None` outside of a turn
    /// in a channel, which has no origin to record.
    pub fn remember(&mut self, text: String) -> Option<MemoryId> {
        let origin = self.turn.classification()?;
        let id = self.turn.agent_state().reserve_memory_id();
        self.turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Remember { id, text }, origin });
        Some(id)
    }

    /// Forgets entry `id` if it may be recalled here or was remembered
    /// earlier in this turn, and is not forgotten already - taken over when
    /// the turn completes. Returns whether it was.
    pub fn forget(&mut self, id: MemoryId) -> bool {
        let Some(origin) = self.turn.classification() else { return false };
        let recalled = Self::recalled(self.turn).iter().any(|m| m.id == id);
        let staged = |wanted: fn(&MemoryChange, MemoryId) -> bool| self.turn.staged_agent_changes().iter()
            .any(|c| matches!(c, AgentStateChange::Memory { change, .. } if wanted(change, id)));
        let remembered_now = staged(|c, id| matches!(c, MemoryChange::Remember { id: r, .. } if *r == id));
        let forgotten = staged(|c, id| matches!(c, MemoryChange::Forget { id: f } if *f == id));
        if !(recalled || remembered_now) || forgotten {
            return false;
        }
        self.turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Forget { id }, origin });
        true
    }
}
