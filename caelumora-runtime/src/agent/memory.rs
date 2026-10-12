//! What an agent remembers across its channels and sessions (issue 62),
//! read and changed within a turn.
//!
//! The memory is part of the agent's `AgentState`; `Turn` offers only
//! plain access to it - a snapshot, staging `MemoryChange`s that
//! `commit` applies when the turn completes. [`AgentMemory`] is the
//! logic on top: which entries may
//! be recalled in the turn's channel - as the policy decides, asked in the
//! agent's name (issue 131) - remembering, updating, forgetting and
//! reclassifying. The
//! `agent_memory` component uses it for its tools and prompt part; anything
//! else that works with an agent's memory in a turn uses it too. What it
//! needs of the session - its privacy, and that of the sessions entries
//! come from - it reads through the turn's views (`Turn::session`, issue
//! 163).

use crate::agent::turn::Turn;
use crate::agent::work::AgentWork;
use crate::tool::{ToolError, ToolResult};
use caelumora_model::actor::ActorId;
use caelumora_model::digest::Digest;
use caelumora_model::confidentiality::Classification;
use caelumora_model::ledger::Staged;
use caelumora_model::memory::{Memories, Memory, MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::session::SessionId;
use crate::view::Views;
use tracing::warn;
use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions, Place};
use caelumora_model::security::{ActionType, Enforcement, Reclassified, SecurityContext, SecurityManager};

/// An agent's memory as a turn sees it: every function takes the turn.
pub struct AgentMemory;

impl AgentMemory {
    /// What may be recalled in `turn`'s channel: entries written in a
    /// channel it may read, as the memory is now - without the turn's own
    /// changes. Whether it may is asked of `confidentiality` in the work's context -
    /// the agent's, with its owner's rights (`ConfidentialityQuestions::may_copy_information`,
    /// issues 131, 255), once per
    /// channel, classification and session the entries come with; the
    /// turn's channel is judged by the classification the turn began with.
    ///
    /// An entry of another session (issue 134) is recalled only where both
    /// the privacy its session had when it was written and the one it has
    /// now permit it - looked up through the turn's views. Of a session
    /// that is gone, the one it had decides; one that cannot be looked up
    /// recalls nothing. A turn in no session recalls nothing.
    pub async fn recalled(work: &AgentWork<'_>, confidentiality: &dyn ConfidentialityPolicy) -> Vec<Memory> {
        let context = work.security_context();
        let Some(own) = work.session().await else { return Vec::new() };
        let Some(here) = own.views().viewer().place().cloned() else { return Vec::new() };
        let memory = work.agent_state().current().memory.current();
        let mut sessions: Vec<(SessionId, Option<SessionPrivacy>)> = Vec::new();
        let mut decided: Vec<(MemoryOrigin, Classification, bool)> = Vec::new();
        let mut recalled = Vec::new();
        for entry in memory {
            let known = decided.iter()
                .find(|(origin, c, _)| *origin == entry.origin && *c == entry.classification)
                .map(|(.., readable)| *readable);
            let readable = match known {
                Some(readable) => readable,
                None => {
                    let readable = Self::may_recall(&entry, &here, own.views(), &mut sessions, confidentiality, context).await;
                    decided.push((entry.origin.clone(), entry.classification.clone(), readable));
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
        here: &Place,
        views: &Views,
        sessions: &mut Vec<(SessionId, Option<SessionPrivacy>)>,
        confidentiality: &dyn ConfidentialityPolicy,
        context: &SecurityContext,
    ) -> bool {
        let origin = &entry.origin;
        // Where it was written, with its session as privacy sees it then or now.
        let source = |session: SessionPrivacy| Place { channel: origin.channel, classification: entry.classification.clone(), session };
        if origin.session.id == here.session.id {
            return confidentiality.may_copy_information(context, &source(here.session.clone()), here);
        }
        if !confidentiality.may_copy_information(context, &source(origin.session.clone()), here) {
            return false;
        }
        let id = origin.session.id;
        let current = match sessions.iter().find(|(known, _)| *known == id) {
            Some((_, current)) => current.clone(),
            None => match views.privacy_of(id).await {
                Ok(current) => {
                    sessions.push((id, current.clone()));
                    current
                }
                Err(e) => {
                    warn!("Looking up session {} to recall from failed: {}", id, e);
                    return false;
                }
            },
        };
        match current {
            Some(current) => confidentiality.may_copy_information(context, &source(current), here),
            None => true,
        }
    }

    /// The digest of the memory as it is now: the state of knowledge a turn
    /// that recalls from it records.
    pub fn digest(work: &AgentWork) -> Option<Digest> {
        work.agent_state().current().memory.digest()
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
    pub async fn remember(turn: &mut Turn<'_>, text: String) -> MemoryId {
        let classification = turn.label();
        let id = turn.agent_state().reserve_memory_id();
        let origin = Self::origin(turn).await;
        turn.stage_memory(MemoryChange::Add { id, text, classification }, origin);
        id
    }

    /// The memory as it stands with the changes of the current turn in it
    /// (issue 156) - every entry, recallable here or not.
    pub fn effective(turn: &Turn) -> Staged<Memories> {
        let staged = turn.staged_memory().iter().map(|(change, origin)| (change, origin.clone()));
        turn.agent_state().current().memory.with_staged(staged)
    }

    /// Entry `id` as it stands with the turn's changes, if it does and is
    /// the agent's to change here: remembered earlier in this turn, or one
    /// that may be recalled here - asked of the policy for this entry alone,
    /// as in [`recalled`](Self::recalled).
    pub async fn find(turn: &Turn<'_>, id: MemoryId, confidentiality: &dyn ConfidentialityPolicy) -> Option<Memory> {
        let entry = Self::effective(turn).get(id)?.clone();
        let Some(stored) = turn.agent_state().current().memory.current().into_iter().find(|m| m.id == id) else {
            return Some(entry);
        };
        let own = turn.session().await?;
        let here = own.views().viewer().place()?.clone();
        Self::may_recall(&stored, &here, own.views(), &mut Vec::new(), confidentiality, turn.security_context()).await.then_some(entry)
    }

    /// Updates entry `id` - one the turn may change (see
    /// [`find`](Self::find)) - to say `text` when the turn completes (issue
    /// 302): classified as the turn has seen so far and written in its
    /// channel, as an entry it remembers is, and recalled from then on where
    /// one remembered here would be. Returns whether there was one.
    pub async fn update(turn: &mut Turn<'_>, id: MemoryId, text: String, confidentiality: &dyn ConfidentialityPolicy) -> bool {
        if Self::find(turn, id, confidentiality).await.is_none() {
            return false;
        }
        let classification = turn.label();
        let origin = Self::origin(turn).await;
        turn.stage_memory(MemoryChange::Update { id, text, classification }, origin);
        true
    }

    /// Forgets entry `id` - one the turn may change (see
    /// [`find`](Self::find)) - when the turn completes. Returns whether
    /// there was one.
    pub async fn forget(turn: &mut Turn<'_>, id: MemoryId, confidentiality: &dyn ConfidentialityPolicy) -> bool {
        if Self::find(turn, id, confidentiality).await.is_none() {
            return false;
        }
        let origin = Self::origin(turn).await;
        turn.stage_memory(MemoryChange::Remove { id }, origin);
        true
    }

    /// Reclassifies entry `id` as `classification` (issue 133) - one the
    /// turn may change (see [`find`](Self::find)) - if the policy lets the
    /// agent (`ActionType::Reclassify`, asked in the turn's context):
    /// never above the turn's channel. Taken over when the turn completes;
    /// from then on the entry is recalled where its new classification may
    /// be read.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such entry here, `PermissionDenied`
    /// if the policy refuses.
    pub async fn reclassify(turn: &mut Turn<'_>, id: MemoryId, classification: Classification, security: &dyn SecurityManager, confidentiality: &dyn ConfidentialityPolicy) -> ToolResult<()> {
        let here = turn.classification();
        if Self::find(turn, id, confidentiality).await.is_none() {
            return Err(ToolError::InvalidParameter(format!("there is no entry #{id} you remember here")));
        }
        let refused = || ToolError::PermissionDenied(format!("reclassifying entry #{id} as {classification}"));
        let attributes = turn.work().channel_attributes();
        let action = ActionType::Reclassify(Reclassified::Memory { author: ActorId::Agent(turn.agent_id()), attributes: &attributes });
        security.require(turn.security_context(), action, |_| refused()).await?;
        confidentiality.require_publish(turn.security_context(), &classification, &here, refused)?;
        let origin = Self::origin(turn).await;
        turn.stage_memory(MemoryChange::Reclassify { id, classification }, origin);
        Ok(())
    }

    /// Where a change made in `turn` is made: its channel, and the privacy
    /// its session has now (issue 134) - private, for a turn in no session.
    async fn origin(turn: &Turn<'_>) -> MemoryOrigin {
        let session = turn.session().await.map(|s| s.privacy())
            .unwrap_or_else(|| SessionPrivacy { id: turn.session_id(), ..SessionPrivacy::default() });
        MemoryOrigin { channel: turn.channel_id(), session }
    }
}
