//! An agent's notes in a channel (issue 17) and its private notes there
//! (issue 51), read and changed within a turn.
//!
//! Both are chains in the agent's `AgentChannelState`; `Turn` offers only
//! plain access to them - reading them, staging `NoteChange`s that
//! `commit` pushes when the turn completes (issue 65). [`AgentNotes`]
//! is the logic on top: numbering, noting, updating, revoking, reclassifying - each
//! looking at the notes as they stand with the turn's own changes in them
//! ([`AgentNotes::effective`], issue 156). The `notes` and `private_notes`
//! components use it; anything else that works with an agent's notes in a
//! turn uses it too.

use crate::turn::Turn;
use crate::work::AgentWork;
use crate::tool::{ToolError, ToolResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::turn::TurnState;
use caelumora_model::confidentiality::Classification;
use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::security::{ActionType, Enforcement, Reclassified, AccessPolicy};
use caelumora_model::digest::Digest;
use caelumora_model::ledger::Staged;
use caelumora_model::note::{Note, NoteChain, NoteChange, NoteId, NoteOrigin, Notes};

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

    /// The changes to this chain staged in `turn`.
    pub(crate) fn staged(self, turn: &TurnState) -> &[NoteChange] {
        match self {
            NoteKind::Public => &turn.notes,
            NoteKind::Private => &turn.private_notes,
        }
    }

    pub(crate) fn staged_mut(self, turn: &mut TurnState) -> &mut Vec<NoteChange> {
        match self {
            NoteKind::Public => &mut turn.notes,
            NoteKind::Private => &mut turn.private_notes,
        }
    }
}

/// An agent's note chains in a channel, as a turn sees them: every function
/// takes the turn and which chain.
pub struct AgentNotes;

impl AgentNotes {
    /// The notes as they stand - without the changes of the current turn,
    /// which count only once it completes.
    pub fn current(work: &AgentWork, kind: NoteKind) -> Vec<Note> {
        kind.chain(work.agent_channel_state()).current()
    }

    /// The digest of the chain, `None` while it is empty: the state of
    /// knowledge a reader of the notes records.
    pub fn digest(work: &AgentWork, kind: NoteKind) -> Option<Digest> {
        kind.chain(work.agent_channel_state()).digest()
    }

    /// The notes as they stand with the changes of the current turn in
    /// them (issue 156): what the turn's operations on notes look at.
    pub fn effective(turn: &Turn, kind: NoteKind) -> Staged<Notes> {
        let staged = turn.staged_notes(kind).iter().map(|c| (c, NoteOrigin::default()));
        kind.chain(turn.agent_channel_state()).with_staged(staged)
    }

    /// Notes `text` in this turn, classified as the turn has seen so far
    /// (`Turn::label`, issue 168), and returns the note's number: one more
    /// than any in the chain or noted earlier in the turn.
    pub fn note(turn: &mut Turn, kind: NoteKind, text: String) -> NoteId {
        let id = Self::effective(turn, kind).next_id();
        let classification = turn.label();
        turn.stage_note(kind, NoteChange::Add { id, text, classification });
        id
    }

    /// Updates note `id` - one of the current notes, or one noted earlier in
    /// this turn - to say `text` when the turn completes (issue 302),
    /// classified as the turn has seen so far, as a new note is. `false` if
    /// there is no such note (or it is revoked).
    pub fn update(turn: &mut Turn, kind: NoteKind, id: NoteId, text: String) -> bool {
        if Self::effective(turn, kind).get(id).is_none() {
            return false;
        }
        let classification = turn.label();
        turn.stage_note(kind, NoteChange::Update { id, text, classification });
        true
    }

    /// Revokes note `id` - one of the current notes, or one noted earlier in
    /// this turn - when the turn completes. `false` if there is no such note
    /// (or it is revoked already).
    pub fn revoke(turn: &mut Turn, kind: NoteKind, id: NoteId) -> bool {
        if Self::effective(turn, kind).get(id).is_none() {
            return false;
        }
        turn.stage_note(kind, NoteChange::Remove { id });
        true
    }

    /// Reclassifies note `id` as `classification` (issue 133) - one of the
    /// current notes, or one noted earlier in this turn, and not revoked -
    /// if the policy lets the agent (`ActionType::Reclassify`, asked in the
    /// turn's context): never above the channel. Taken over when the turn
    /// completes.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such note, `PermissionDenied` if
    /// the policy refuses.
    pub async fn reclassify(turn: &mut Turn<'_>, kind: NoteKind, id: NoteId, classification: Classification, access_policy: &dyn AccessPolicy, confidentiality: &dyn ConfidentialityPolicy) -> ToolResult<()> {
        if Self::effective(turn, kind).get(id).is_none() {
            return Err(ToolError::InvalidParameter(format!("you have no note #{id}; only your own current notes can be reclassified")));
        }
        let channel = turn.classification();
        let refused = || ToolError::PermissionDenied(format!("reclassifying note #{id} as {classification}"));
        let attributes = turn.work().channel_attributes();
        let action = ActionType::Reclassify(Reclassified::Note { author: ActorId::Agent(turn.agent_id()), attributes: &attributes });
        access_policy.require(turn.security_context(), action, |_| refused()).await?;
        confidentiality.require_publish(turn.security_context(), &classification, &channel, refused)?;
        turn.stage_note(kind, NoteChange::Reclassify { id, classification });
        Ok(())
    }
}
