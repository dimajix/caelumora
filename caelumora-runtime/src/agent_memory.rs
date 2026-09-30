//! What an agent remembers across its channels and sessions (issue 62),
//! read and changed within a turn.
//!
//! The memory is part of the agent's `AgentState`; `Turn` offers only
//! generic access to that state - a snapshot, staged changes - and knows
//! nothing of memory. [`AgentMemory`] is the logic on top: which entries may
//! be recalled in the turn's channel - as the policy decides, asked in the
//! agent's name (issue 131) - remembering, forgetting and reclassifying. The
//! `agent_memory` component uses it for its tools and prompt part; anything
//! else that works with an agent's memory in a turn uses it too.

use crate::agent_turn::Turn;
use crate::tool::{ToolError, ToolResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentStateChange;
use caelumora_model::digest::Digest;
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::memory::{Memory, MemoryChange, MemoryId};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, Place, Reclassified, SecurityContext, SecurityManager};

/// An agent's memory as a turn sees it: every function takes the turn.
pub struct AgentMemory;

impl AgentMemory {
    /// What may be recalled in `turn`'s channel, of the session `session`:
    /// entries written in a channel it may read, as the memory is now -
    /// without the turn's own changes. Whether it may is asked of
    /// `security` in `context`, the agent's
    /// (`AccessQuestions::may_channel_read`, issue 131), once per channel
    /// and classification the entries come with; the turn's channel is
    /// judged by the classification the turn began with. An entry of
    /// another session is judged as written in a private one, which no
    /// other session reads (issue 134).
    pub async fn recalled(turn: &Turn<'_>, session: &SessionPrivacy, security: &dyn SecurityManager, context: &SecurityContext) -> Vec<Memory> {
        let (here, classification) = (turn.channel(), turn.classification());
        let reader = Place { channel: &here, classification: &classification, session };
        let elsewhere = SessionPrivacy::default();
        let memory = turn.agent_state().current().memory.current();
        let mut decided: Vec<(ChannelRef, Classification, bool)> = Vec::new();
        let mut recalled = Vec::new();
        for entry in memory {
            let readable = match decided.iter().find(|(origin, c, _)| *origin == entry.origin && *c == entry.classification) {
                Some((_, _, readable)) => *readable,
                None => {
                    let origin_session = if entry.origin.session_id == here.session_id { session } else { &elsewhere };
                    let source = Place { channel: &entry.origin, classification: &entry.classification, session: origin_session };
                    let readable = security.may_channel_read(context, reader, source).await;
                    decided.push((entry.origin, entry.classification.clone(), readable));
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
        let (origin, classification) = (turn.channel(), turn.classification());
        let id = turn.agent_state().reserve_memory_id();
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Remember { id, text, classification }, origin });
        id
    }

    /// Forgets entry `id` if it may be recalled here, in `session` (asked
    /// as in [`recalled`](Self::recalled)) or was remembered earlier in this turn,
    /// and is not forgotten already - taken over when the turn completes.
    /// Returns whether it was.
    pub async fn forget(turn: &mut Turn<'_>, id: MemoryId, session: &SessionPrivacy, security: &dyn SecurityManager, context: &SecurityContext) -> bool {
        let origin = turn.channel();
        let recalled = Self::recalled(turn, session, security, context).await.iter().any(|m| m.id == id);
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

    /// Reclassifies entry `id` as `classification` (issue 133) - one that
    /// may be recalled here (asked as in [`recalled`](Self::recalled)) or
    /// was remembered earlier in this turn, and is not forgotten - if the
    /// policy lets the agent (`ActionType::Reclassify`, asked in `context`,
    /// the agent's): never above the turn's channel. Taken over when the
    /// turn completes; from then on the entry is recalled where its new
    /// classification may be read.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such entry here, `PermissionDenied`
    /// if the policy refuses.
    pub async fn reclassify(turn: &mut Turn<'_>, id: MemoryId, classification: Classification, session: &SessionPrivacy, security: &dyn SecurityManager, context: &SecurityContext) -> ToolResult<()> {
        let (origin, here) = (turn.channel(), turn.classification());
        let recalled = Self::recalled(turn, session, security, context).await.into_iter().find(|m| m.id == id).map(|m| m.classification);
        let staged = turn.staged_agent_changes();
        let remembered_now = staged.iter().any(|c| matches!(c, AgentStateChange::Memory { change: MemoryChange::Remember { id: r, .. }, .. } if *r == id));
        let forgotten = staged.iter().any(|c| matches!(c, AgentStateChange::Memory { change: MemoryChange::Forget { id: f }, .. } if *f == id));
        // What the turn staged last for the entry is what it is now.
        let reclassified_now = staged.iter().rev().find_map(|c| match c {
            AgentStateChange::Memory { change: MemoryChange::Reclassify { id: r, classification }, .. } if *r == id => Some(classification.clone()),
            _ => None,
        });
        let current = reclassified_now.or(recalled).or_else(|| remembered_now.then(|| here.clone()));
        let Some(current) = current.filter(|_| !forgotten) else {
            return Err(ToolError::InvalidParameter(format!("there is no entry #{id} you remember here")));
        };
        let action = ActionType::Reclassify {
            target: Reclassified::Memory { author: ActorId::Agent(turn.agent_id()) },
            current: &current,
            new: &classification,
            channel: &here,
        };
        security.require(context, action, || ToolError::PermissionDenied(format!("reclassifying entry #{id} as {classification}"))).await?;
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Reclassify { id, classification }, origin });
        Ok(())
    }
}
