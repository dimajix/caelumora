//! Notes (issue 17): what an agent writes down in a channel, for itself and
//! for others to read.
//!
//! Notes belong to the agent that wrote them and are published in the
//! channel they were written in (principles 2, 3 and 3a in issue 19). Each
//! agent keeps them per channel as a [`NoteChain`], a chain of changes that
//! is only appended to: a note, or the revocation of an earlier one. A new
//! version of a note is a revocation and a new note. What is noted now is
//! the chain's active branch read from start to end (issue 08).

use crate::chain::{Chain, ChainEntry};
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::MessageId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The number of a note in its chain: 1 for the first, counting on. Small,
/// because a model has to repeat it to revoke the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(pub u32);

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A change to an agent's notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteChange {
    /// A note, classified as the channel it was written in was then (issue
    /// 133): what the agent derives carries its channel's classification,
    /// and a later change of the channel changes none of its notes.
    Note { id: NoteId, text: String, classification: Classification },
    Revoke { id: NoteId },
    /// The note `id` is classified `classification` from now on (issue
    /// 133): released, or raised.
    Reclassify { id: NoteId, classification: Classification },
}

/// An entry of a [`NoteChain`]: a change, where in the conversation it was
/// made, and the digest of the chain up to and including it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteEntry {
    pub change: NoteChange,
    /// The message the change follows: the last one of the turn that made
    /// it (`MessageId::default()` if there was none).
    pub after: MessageId,
    /// The digest of the agent's transcript when the change was taken over.
    pub transcript: Option<Digest>,
    pub written_at: DateTime<Utc>,
    pub digest: Digest,
}

impl ChainEntry for NoteEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// A note as it stands now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub text: String,
    /// Its channel's classification when it was written (issue 133).
    pub classification: Classification,
    /// The message it follows.
    pub after: MessageId,
    pub written_at: DateTime<Utc>,
}

/// An agent's notes in one channel, as a chain of changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteChain {
    changes: Chain<NoteEntry>,
}

impl NoteChain {
    /// The digest of the whole chain - what a reader records as the state
    /// of knowledge it saw - or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.changes.digest()
    }

    /// The changes of the active branch.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &NoteEntry> + ExactSizeIterator {
        self.changes.entries()
    }

    /// Appends `change`, made in the turn that ended with message `after`
    /// while the agent's transcript had the digest `transcript`.
    pub fn push(&mut self, change: NoteChange, after: MessageId, transcript: Option<Digest>) {
        self.changes.push_with(|previous| {
            let digest = crate::digest::note_chain(previous, &change, after);
            NoteEntry { change, after, transcript, written_at: Utc::now(), digest }
        });
    }

    /// The notes as they stand on the active branch: every note that was
    /// not revoked, oldest first.
    pub fn current(&self) -> Vec<Note> {
        let mut notes: Vec<Note> = Vec::new();
        for entry in self.changes.entries() {
            match &entry.change {
                NoteChange::Note { id, text, classification } => notes.push(Note {
                    id: *id,
                    text: text.clone(),
                    classification: classification.clone(),
                    after: entry.after,
                    written_at: entry.written_at,
                }),
                NoteChange::Revoke { id } => notes.retain(|n| n.id != *id),
                NoteChange::Reclassify { id, classification } => {
                    if let Some(note) = notes.iter_mut().find(|n| n.id == *id) {
                        note.classification = classification.clone();
                    }
                }
            }
        }
        notes
    }

    /// The number the next note gets: one more than any note ever written,
    /// on any branch, so that a number is never given again - a reader may
    /// have read it before it was revoked, or on a branch no longer active.
    pub fn next_id(&self) -> NoteId {
        let highest = self.changes.all_entries()
            .filter_map(|e| match &e.change {
                NoteChange::Note { id, .. } => Some(id.0),
                NoteChange::Revoke { .. } | NoteChange::Reclassify { .. } => None,
            })
            .max()
            .unwrap_or(0);
        NoteId(highest + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u32, text: &str) -> NoteChange {
        NoteChange::Note { id: NoteId(id), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT }
    }

    #[test]
    fn what_is_noted_is_the_chain_read_from_start_to_end() {
        let mut chain = NoteChain::default();
        assert_eq!(chain.next_id(), NoteId(1));
        chain.push(note(1, "The user likes tea."), MessageId::new(), None);
        chain.push(note(2, "The user lives in Berlin."), MessageId::new(), None);
        chain.push(NoteChange::Revoke { id: NoteId(1) }, MessageId::new(), None);
        chain.push(note(3, "The user likes coffee."), MessageId::new(), None);

        let texts: Vec<String> = chain.current().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, ["The user lives in Berlin.", "The user likes coffee."]);
        assert_eq!(chain.next_id(), NoteId(4));
    }

    #[test]
    fn a_reclassified_note_keeps_its_number_and_text() {
        use crate::confidentiality::Confidentiality;
        let mut chain = NoteChain::default();
        chain.push(note(1, "a"), MessageId::new(), None);
        let released = Classification::general(Confidentiality::Public);
        chain.push(NoteChange::Reclassify { id: NoteId(1), classification: released.clone() }, MessageId::new(), None);
        let current = chain.current();
        assert_eq!((current[0].id, current[0].text.as_str(), &current[0].classification), (NoteId(1), "a", &released));
        assert_eq!(chain.next_id(), NoteId(2));
    }

    #[test]
    fn a_revoked_number_is_not_given_again() {
        let mut chain = NoteChain::default();
        chain.push(note(1, "a"), MessageId::new(), None);
        chain.push(NoteChange::Revoke { id: NoteId(1) }, MessageId::new(), None);
        assert!(chain.current().is_empty());
        assert_eq!(chain.next_id(), NoteId(2));
    }

    #[test]
    fn a_branch_no_longer_active_is_not_noted_but_its_numbers_are_not_given_again() {
        let mut chain = NoteChain::default();
        chain.push(note(1, "a"), MessageId::new(), None);
        let fork = chain.digest();
        chain.push(note(2, "b"), MessageId::new(), None);
        chain.push(NoteChange::Revoke { id: NoteId(1) }, MessageId::new(), None);
        chain.changes.move_head(fork.as_ref());

        let texts: Vec<String> = chain.current().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, ["a"], "the revocation was on the other branch too");
        assert_eq!(chain.next_id(), NoteId(3), "2 was given on the other branch");
    }

    #[test]
    fn the_digest_covers_revocations_and_the_place_in_the_conversation() {
        let after = MessageId::new();
        let mut noted = NoteChain::default();
        noted.push(note(1, "a"), after, None);
        let before_revocation = noted.digest();
        noted.push(NoteChange::Revoke { id: NoteId(1) }, after, None);
        assert_ne!(noted.digest(), before_revocation);

        let mut elsewhere = NoteChain::default();
        elsewhere.push(note(1, "a"), MessageId::new(), None);
        assert_ne!(elsewhere.digest(), before_revocation, "the same note after another message");

        let loaded: NoteChain = serde_json::from_value(serde_json::to_value(&noted).unwrap()).unwrap();
        assert_eq!(loaded.digest(), noted.digest());
        assert_eq!(loaded.current(), noted.current());
    }
}
