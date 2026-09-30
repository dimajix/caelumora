//! What an agent remembers across its channels and sessions (issue 62),
//! read and changed within a turn.
//!
//! The memory is part of the agent's `AgentState`; `Turn` offers only
//! generic access to that state - a snapshot, staged changes - and knows
//! nothing of memory. [`AgentMemory`] is the logic on top: which entries may
//! be recalled in the turn's channel - as the policy decides, asked in the
//! agent's name (issue 131) - remembering and forgetting. The
//! `agent_memory` component uses it for its tools and prompt part; anything
//! else that works with an agent's memory in a turn uses it too.

use crate::agent_turn::Turn;
use caelumora_model::agent::AgentStateChange;
use caelumora_model::digest::Digest;
use caelumora_model::confidentiality::ChannelClassification;
use caelumora_model::memory::{Memory, MemoryChange, MemoryId};
use caelumora_model::security::{AccessQuestions, SecurityContext, SecurityManager};

/// An agent's memory as a turn sees it: every function takes the turn.
pub struct AgentMemory;

impl AgentMemory {
    /// What may be recalled in `turn`'s channel: entries written in a
    /// channel it may read, as the memory is now - without the turn's own
    /// changes. Whether it may is asked of `security` in `context`, the
    /// agent's (`AccessQuestions::may_channel_read`, issue 131), once per
    /// channel the entries come from; the turn's channel is judged by the
    /// classification the turn began with.
    pub async fn recalled(turn: &Turn<'_>, security: &dyn SecurityManager, context: &SecurityContext) -> Vec<Memory> {
        let here = turn.classification();
        let memory = turn.agent_state().current().memory.current();
        let mut decided: Vec<(ChannelClassification, bool)> = Vec::new();
        let mut recalled = Vec::new();
        for entry in memory {
            let readable = match decided.iter().find(|(origin, _)| *origin == entry.origin) {
                Some((_, readable)) => *readable,
                None => {
                    let readable = security.may_channel_read(context, &here, &entry.origin).await;
                    decided.push((entry.origin, readable));
                    readable
                }
            };
            if readable {
                recalled.push(entry);
            }
        }
        recalled
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

    /// Forgets entry `id` if it may be recalled here (asked as in
    /// [`recalled`](Self::recalled)) or was remembered earlier in this turn,
    /// and is not forgotten already - taken over when the turn completes.
    /// Returns whether it was.
    pub async fn forget(turn: &mut Turn<'_>, id: MemoryId, security: &dyn SecurityManager, context: &SecurityContext) -> bool {
        let origin = turn.classification();
        let recalled = Self::recalled(turn, security, context).await.iter().any(|m| m.id == id);
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
