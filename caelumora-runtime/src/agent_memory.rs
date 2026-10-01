//! What an agent remembers across its channels and sessions (issue 62),
//! read and changed within a turn.
//!
//! The memory is part of the agent's `AgentState`; `Turn` offers only
//! generic access to that state - a snapshot, staged changes - and knows
//! nothing of memory. [`AgentMemory`] is the logic on top: which entries may
//! be recalled in the turn's channel - as the policy decides, asked in the
//! agent's name (issue 131) - remembering, forgetting and reclassifying. The
//! `agent_memory` component uses it for its tools and prompt part; anything
//! else that works with an agent's memory in a turn uses it too. What it
//! needs of the session - its privacy, and that of the sessions entries
//! come from - it reads through the turn's views (`Turn::session`, issue
//! 163).

use crate::agent_turn::Turn;
use crate::tool::{ToolError, ToolResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentStateChange;
use caelumora_model::digest::Digest;
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::ledger::Staged;
use caelumora_model::memory::{Memories, Memory, MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::session::SessionId;
use crate::view::Views;
use tracing::warn;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, Place, Reclassified, SecurityContext, SecurityManager};

/// An agent's memory as a turn sees it: every function takes the turn.
pub struct AgentMemory;

impl AgentMemory {
    /// What may be recalled in `turn`'s channel: entries written in a
    /// channel it may read, as the memory is now - without the turn's own
    /// changes. Whether it may is asked of `security` in `context`, the
    /// agent's (`AccessQuestions::may_channel_read`, issue 131), once per
    /// channel, classification and session the entries come with; the
    /// turn's channel is judged by the classification the turn began with.
    ///
    /// An entry of another session (issue 134) is recalled only where both
    /// the privacy its session had when it was written and the one it has
    /// now permit it - looked up through the turn's views. Of a session
    /// that is gone, the one it had decides; one that cannot be looked up
    /// recalls nothing. A turn in no session recalls nothing.
    pub async fn recalled(turn: &Turn<'_>, security: &dyn SecurityManager, context: &SecurityContext) -> Vec<Memory> {
        let Some(own) = turn.session() else { return Vec::new() };
        let (here, classification, session) = (turn.channel(), turn.classification(), own.privacy());
        let place = Place { channel: &here, classification: &classification, session: &session };
        let memory = turn.agent_state().current().memory.current();
        let mut sessions: Vec<(SessionId, Option<SessionPrivacy>)> = Vec::new();
        let mut decided: Vec<(ChannelRef, Classification, SessionPrivacy, bool)> = Vec::new();
        let mut recalled = Vec::new();
        for entry in memory {
            let known = decided.iter()
                .find(|(origin, c, s, _)| *origin == entry.origin.channel && *c == entry.classification && *s == entry.origin.session)
                .map(|(.., readable)| *readable);
            let readable = match known {
                Some(readable) => readable,
                None => {
                    let readable = Self::may_recall(&entry, place, own.views(), &mut sessions, security, context).await;
                    decided.push((entry.origin.channel, entry.classification.clone(), entry.origin.session.clone(), readable));
                    readable
                }
            };
            if readable {
                recalled.push(entry);
            }
        }
        recalled
    }

    /// Whether `entry` may be recalled in the channel `here` (see
    /// [`recalled`](Self::recalled)). `sessions` holds the privacy of the
    /// other sessions looked up so far - `None` for one that is gone.
    async fn may_recall(
        entry: &Memory,
        here: Place<'_>,
        views: &Views,
        sessions: &mut Vec<(SessionId, Option<SessionPrivacy>)>,
        security: &dyn SecurityManager,
        context: &SecurityContext,
    ) -> bool {
        let (origin, classification) = (&entry.origin.channel, &entry.classification);
        if origin.session_id == here.channel.session_id {
            let source = Place { channel: origin, classification, session: here.session };
            return security.may_channel_read(context, here, source).await;
        }
        let then = Place { channel: origin, classification, session: &entry.origin.session };
        if !security.may_channel_read(context, here, then).await {
            return false;
        }
        let current = match sessions.iter().find(|(id, _)| *id == origin.session_id) {
            Some((_, current)) => current.clone(),
            None => match views.privacy_of(origin.session_id).await {
                Ok(current) => {
                    sessions.push((origin.session_id, current.clone()));
                    current
                }
                Err(e) => {
                    warn!("Looking up session {} to recall from failed: {}", origin.session_id, e);
                    return false;
                }
            },
        };
        match current {
            Some(current) => security.may_channel_read(context, here, Place { channel: origin, classification, session: &current }).await,
            None => true,
        }
    }

    /// The digest of the memory as it is now: the state of knowledge a turn
    /// that recalls from it records.
    pub fn digest(turn: &Turn) -> Option<Digest> {
        turn.agent_state().current().memory.digest()
    }

    /// Remembers `text`, as written in the turn's channel, with the privacy
    /// its session has now (issue 134) and classified as the turn has seen
    /// so far (`Turn::label`, issue 168): taken over when the turn completes.
    /// Returns its number.
    ///
    /// Unlike a note's, the number is reserved on the agent's shared state
    /// handle (`AgentStateHandle::reserve_memory_id`) instead of being
    /// derived from the memory and the turn's staged changes: several actors
    /// of one agent - in other channels and sessions - remember at the same
    /// time, and would otherwise hand out the same number. Deliberately the
    /// one exception to "commit, don't write" (issue 19).
    pub fn remember(turn: &mut Turn, text: String) -> MemoryId {
        let classification = turn.label();
        let id = turn.agent_state().reserve_memory_id();
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Add { id, text, classification }, origin: Self::origin(turn) });
        id
    }

    /// The memory as it stands with the changes of the current turn in it
    /// (issue 156) - every entry, recallable here or not.
    pub fn effective(turn: &Turn) -> Staged<Memories> {
        let staged = turn.staged_agent_changes().iter().map(|AgentStateChange::Memory { change, origin }| (change, origin.clone()));
        turn.agent_state().current().memory.with_staged(staged)
    }

    /// Entry `id` as it stands with the turn's changes, if it does and is
    /// the agent's to change here: remembered earlier in this turn, or one
    /// that may be recalled here - asked of the policy for this entry alone,
    /// as in [`recalled`](Self::recalled).
    pub async fn find(turn: &Turn<'_>, id: MemoryId, security: &dyn SecurityManager, context: &SecurityContext) -> Option<Memory> {
        let entry = Self::effective(turn).get(id)?.clone();
        let Some(stored) = turn.agent_state().current().memory.current().into_iter().find(|m| m.id == id) else {
            return Some(entry);
        };
        let own = turn.session()?;
        let (here, classification, session) = (turn.channel(), turn.classification(), own.privacy());
        let place = Place { channel: &here, classification: &classification, session: &session };
        Self::may_recall(&stored, place, own.views(), &mut Vec::new(), security, context).await.then_some(entry)
    }

    /// Forgets entry `id` - one the turn may change (see
    /// [`find`](Self::find)) - when the turn completes. Returns whether
    /// there was one.
    pub async fn forget(turn: &mut Turn<'_>, id: MemoryId, security: &dyn SecurityManager, context: &SecurityContext) -> bool {
        if Self::find(turn, id, security, context).await.is_none() {
            return false;
        }
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Remove { id }, origin: Self::origin(turn) });
        true
    }

    /// Reclassifies entry `id` as `classification` (issue 133) - one the
    /// turn may change (see [`find`](Self::find)) - if the policy lets the
    /// agent (`ActionType::Reclassify`, asked in `context`, the agent's):
    /// never above the turn's channel. Taken over when the turn completes;
    /// from then on the entry is recalled where its new classification may
    /// be read.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such entry here, `PermissionDenied`
    /// if the policy refuses.
    pub async fn reclassify(turn: &mut Turn<'_>, id: MemoryId, classification: Classification, security: &dyn SecurityManager, context: &SecurityContext) -> ToolResult<()> {
        let here = turn.classification();
        let Some(current) = Self::find(turn, id, security, context).await.map(|m| m.classification) else {
            return Err(ToolError::InvalidParameter(format!("there is no entry #{id} you remember here")));
        };
        let action = ActionType::Reclassify {
            target: Reclassified::Memory { author: ActorId::Agent(turn.agent_id()) },
            current: &current,
            new: &classification,
            channel: &here,
        };
        security.require(context, action, || ToolError::PermissionDenied(format!("reclassifying entry #{id} as {classification}"))).await?;
        let origin = Self::origin(turn);
        turn.stage_agent_change(AgentStateChange::Memory { change: MemoryChange::Reclassify { id, classification }, origin });
        Ok(())
    }

    /// Where a change made in `turn` is made: its channel, and the privacy
    /// its session has now (issue 134) - private, for a turn in no session.
    fn origin(turn: &Turn) -> MemoryOrigin {
        MemoryOrigin { channel: turn.channel(), session: turn.session().map(|s| s.privacy()).unwrap_or_default() }
    }
}
