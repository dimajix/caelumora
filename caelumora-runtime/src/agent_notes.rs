//! An agent's notes in a channel (issue 17) and its private notes there
//! (issue 51), read and changed within a turn.
//!
//! Both are chains in the agent's `AgentChannelState`; `Turn` offers
//! only generic access to that state - reading it, staging
//! `AgentChannelChange`s that `commit_turn` applies when the turn completes -
//! and knows nothing of notes (issue 65). [`AgentNotes`] is the logic on top:
//! numbering, noting, revoking. The `notes` and `private_notes` components use
//! it; anything else that works with an agent's notes in a turn uses it too.

use crate::agent_turn::Turn;
use crate::tool::{ToolError, ToolResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelChange, AgentChannelState};
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{ActionType, Enforcement, Reclassified, SecurityContext, SecurityManager};
use caelumora_model::digest::Digest;
use caelumora_model::note::{Note, NoteChain, NoteChange, NoteId};

/// Which of an agent's note chains in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// Published in the channel: its participants read them.
    Public,
    /// Read by the agent alone, numbered independently of the public ones,
    /// so that gaps in those do not reveal private notes.
    Private,
}

impl NoteKind {
    fn chain(self, state: &AgentChannelState) -> &NoteChain {
        match self {
            NoteKind::Public => &state.notes,
            NoteKind::Private => &state.private_notes,
        }
    }

    fn change(self, change: NoteChange) -> AgentChannelChange {
        match self {
            NoteKind::Public => AgentChannelChange::Note(change),
            NoteKind::Private => AgentChannelChange::PrivateNote(change),
        }
    }

    /// `change` if it is a change to this chain.
    fn of(self, change: &AgentChannelChange) -> Option<&NoteChange> {
        match (self, change) {
            (NoteKind::Public, AgentChannelChange::Note(change)) => Some(change),
            (NoteKind::Private, AgentChannelChange::PrivateNote(change)) => Some(change),
            _ => None,
        }
    }
}

/// An agent's note chains in a channel, as a turn sees them: every function
/// takes the turn and which chain.
pub struct AgentNotes;

impl AgentNotes {
    /// The notes as they stand - without the changes of the current turn,
    /// which count only once it completes.
    pub fn current(turn: &Turn, kind: NoteKind) -> Vec<Note> {
        kind.chain(turn.agent_channel_state()).current()
    }

    /// The digest of the chain, `None` while it is empty: the state of
    /// knowledge a reader of the notes records.
    pub fn digest(turn: &Turn, kind: NoteKind) -> Option<Digest> {
        kind.chain(turn.agent_channel_state()).digest()
    }

    /// Notes `text` in this turn and returns the note's number: one more than
    /// any in the chain or noted earlier in the turn.
    pub fn note(turn: &mut Turn, kind: NoteKind, text: String) -> NoteId {
        let noted_now = turn.staged_channel_changes().iter()
            .filter_map(|c| match kind.of(c) {
                Some(NoteChange::Note { id, .. }) => Some(id.0),
                _ => None,
            })
            .max();
        let id = match noted_now {
            Some(highest) => NoteId(highest + 1),
            None => kind.chain(turn.agent_channel_state()).next_id(),
        };
        let classification = turn.classification().classification;
        turn.stage_channel_change(kind.change(NoteChange::Note { id, text, classification }));
        id
    }

    /// Revokes note `id`: one of the current notes, or one noted earlier in
    /// this turn, which is then simply dropped - it was never published.
    /// `false` if there is no such note.
    pub fn revoke(turn: &mut Turn, kind: NoteKind, id: NoteId) -> bool {
        let staged = turn.staged_channel_changes();
        if let Some(index) = staged.iter().position(|c| matches!(kind.of(c), Some(NoteChange::Note { id: noted, .. }) if *noted == id)) {
            staged.remove(index);
            return true;
        }
        let revoked_now = staged.iter().any(|c| matches!(kind.of(c), Some(NoteChange::Revoke { id: revoked }) if *revoked == id));
        if revoked_now || !kind.chain(turn.agent_channel_state()).current().iter().any(|n| n.id == id) {
            return false;
        }
        turn.stage_channel_change(kind.change(NoteChange::Revoke { id }));
        true
    }

    /// Reclassifies note `id` as `classification` (issue 133) - one of the
    /// current notes, or one noted earlier in this turn, and not revoked -
    /// if the policy lets the agent (`ActionType::Reclassify`, asked in
    /// `context`, the agent's): never above the channel. Taken over when the
    /// turn completes.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such note, `PermissionDenied` if
    /// the policy refuses.
    pub async fn reclassify(turn: &mut Turn<'_>, kind: NoteKind, id: NoteId, classification: Classification, security: &dyn SecurityManager, context: &SecurityContext) -> ToolResult<()> {
        let staged = turn.staged_channel_changes();
        let noted_now = staged.iter().rev().find_map(|c| match kind.of(c) {
            Some(NoteChange::Note { id: noted, classification, .. }) if *noted == id => Some(classification.clone()),
            _ => None,
        });
        let revoked_now = staged.iter().any(|c| matches!(kind.of(c), Some(NoteChange::Revoke { id: revoked }) if *revoked == id));
        let current = noted_now.or_else(|| kind.chain(turn.agent_channel_state()).current().into_iter().find(|n| n.id == id).map(|n| n.classification));
        let Some(current) = current.filter(|_| !revoked_now) else {
            return Err(ToolError::InvalidParameter(format!("you have no note #{id}; only your own current notes can be reclassified")));
        };
        let channel = turn.classification();
        let action = ActionType::Reclassify {
            target: Reclassified::Note { author: ActorId::Agent(turn.agent_id()) },
            current: &current,
            new: &classification,
            channel: &channel,
        };
        security.require(context, action, || ToolError::PermissionDenied(format!("reclassifying note #{id} as {classification}"))).await?;
        turn.stage_channel_change(kind.change(NoteChange::Reclassify { id, classification }));
        Ok(())
    }
}
