//! An agent's transcript (issue 50): the chain of messages its context is
//! built from, and the turns it gave up (issue 145).

use crate::chain::{Chain, ChainEntry};
use crate::digest::Digest;
use crate::message::{last_message_id_of, messages_of, push_message, Message, MessageEntry, MessageId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A message of a `Transcript`, with the digest of the transcript up to and
/// including it (issue 06).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub message: Message,
    pub digest: Digest,
    /// Set on the last message of a turn that was given up (issue 145):
    /// what the branch ending here is. Not covered by the digest - it says
    /// something about the branch, not what a model saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub given_up: Option<GivenUp>,
}

/// That a turn was given up rather than taken over (issue 145): its
/// messages stay in the agent's transcript as a branch that is no longer
/// active, ending in the message this is set on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GivenUp {
    pub reason: GivenUpReason,
    pub at: DateTime<Utc>,
    /// How many of the messages up to this one are the turn's.
    pub turn_entries: usize,
}

/// Why a turn was given up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GivenUpReason {
    /// It failed; `message` is what its users were told.
    Failed { message: String },
    /// The channel's classification was lowered to `channel` while the
    /// turn ran (issue 141): what the turn saw may not flow into it any
    /// more.
    Lowered { channel: crate::confidentiality::Classification },
    /// The agent was stopped, leaving the channel: removed from it, or the
    /// channel deleted. Who did that is a matter of that act, not of its
    /// consequence - the audit log's (issue 140).
    Stopped,
}

/// A turn that was given up: the mark, and the turn's messages.
#[derive(Debug, Clone)]
pub struct GivenUpTurn {
    pub given_up: GivenUp,
    pub messages: Vec<Message>,
}

impl ChainEntry for TranscriptEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

impl MessageEntry for TranscriptEntry {
    fn message(&self) -> &Message {
        &self.message
    }
}

/// An agent's transcript (issue 50): the sequence of messages its context is
/// built from - its own messages in full, with tool calls and internal
/// steps, and what others said as it received them. The context of a model
/// call adds the prompt parts and the turn's messages to it; the history is
/// the channel's public record, which the transcript is not.
///
/// A chain: every entry's digest covers its message and the digest before
/// it, so one digest names the whole transcript up to that point by content
/// (see `crate::chain`). Nothing is ever removed from it - also not from a
/// branch no longer active (issue 08) - so every digest it had names a
/// prefix of one of its branches, which can be recovered (`prefix`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Transcript {
    messages: Chain<TranscriptEntry>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole transcript, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.messages.digest()
    }

    pub fn messages(&self) -> Vec<Message> {
        messages_of(&self.messages)
    }

    pub fn last_message_id(&self) -> MessageId {
        last_message_id_of(&self.messages)
    }

    /// Appends `msgs` and returns, for each of them, the digest of the
    /// transcript before it - the transcript it was produced from.
    pub fn extend(&mut self, msgs: Vec<Message>) -> Vec<Option<Digest>> {
        msgs.into_iter().map(|m| self.push(m)).collect()
    }

    /// Appends `msg` and returns the digest of the transcript before it.
    pub fn push(&mut self, msg: Message) -> Option<Digest> {
        push_message(&mut self.messages, msg, |message, digest| TranscriptEntry { message, digest, given_up: None })
    }

    /// Moves the head to the entry with `digest` - `None`: before every
    /// entry - to go on from there (issue 08). `false` if there is none.
    pub fn move_head(&mut self, digest: Option<&Digest>) -> bool {
        self.messages.move_head(digest)
    }

    /// Marks the entry with `digest` as the end of a turn given up (issue
    /// 145). `false` if there is none.
    pub fn mark_given_up(&mut self, digest: &Digest, given_up: GivenUp) -> bool {
        match self.messages.entries_mut().find(|e| &e.digest == digest) {
            Some(entry) => {
                entry.given_up = Some(given_up);
                true
            }
            None => false,
        }
    }

    /// The turns given up, in the order they ended in the transcript, each
    /// with its messages.
    pub fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        self.messages.all_entries()
            .filter_map(|entry| {
                let given_up = entry.given_up.clone()?;
                let path = self.messages.prefix(&entry.digest)?;
                let start = path.len().saturating_sub(given_up.turn_entries);
                let messages = path[start..].iter().map(|e| e.message.clone()).collect();
                Some(GivenUpTurn { given_up, messages })
            })
            .collect()
    }

    /// The entries of the active branch.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &TranscriptEntry> + ExactSizeIterator {
        self.messages.entries()
    }

    /// The entries of the transcript `digest` names: this transcript up to
    /// and including the entry with that digest, on whichever branch, or
    /// `None` if it is not a prefix of this transcript.
    pub fn prefix(&self, digest: &Digest) -> Option<Vec<&TranscriptEntry>> {
        self.messages.prefix(digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::message_chain;
    use crate::message::tests::{id, make_message};

    #[test]
    fn message_context_empty_state() {
        let context = Transcript::new();

        assert_eq!(context.digest(), None);
        assert!(context.messages().is_empty());
    }

    #[test]
    fn message_context_chains_the_digests_of_its_messages() {
        let (m1, m2, m3) = (make_message(id(1)), make_message(id(2)), make_message(id(3)));

        let mut context = Transcript::new();
        let before = context.extend(vec![m1.clone(), m2.clone(), m3.clone()]);

        let d1 = message_chain(None, &m1);
        let d2 = message_chain(Some(&d1), &m2);
        let d3 = message_chain(Some(&d2), &m3);
        assert_eq!(context.digest(), Some(d3));
        assert_eq!(before, vec![None, Some(d1), Some(d2)], "each message with the context it came after");
        assert_eq!(context.messages().iter().map(|m| m.id).collect::<Vec<_>>(), vec![m1.id, m2.id, m3.id]);
    }

    #[test]
    fn message_context_extend_matches_repeated_push_digest() {
        let messages = vec![make_message(id(11)), make_message(id(12)), make_message(id(13))];

        let mut extend_context = Transcript::new();
        extend_context.extend(messages.clone());

        let mut push_context = Transcript::new();
        for message in messages {
            push_context.push(message);
        }

        assert_eq!(extend_context.digest(), push_context.digest());
    }

    #[test]
    fn a_digest_names_the_prefix_it_was_taken_at() {
        let mut context = Transcript::new();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);
        let at_two = context.digest().unwrap();
        context.push(make_message(id(3)));

        let prefix = context.prefix(&at_two).expect("an earlier digest names a prefix");
        assert_eq!(prefix.iter().map(|e| e.message.id).collect::<Vec<_>>(), vec![id(1), id(2)]);
        assert!(context.prefix(&crate::digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn digests_are_kept_as_recorded() {
        let mut context = Transcript::new();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);

        // What was recorded stands, even if today's scheme would compute
        // something else - as it will for anything recorded under an
        // earlier version of it.
        let recorded = crate::digest::prompt("recorded under another scheme");
        let mut stored = serde_json::to_value(&context).unwrap();
        stored["messages"]["entries"][1]["digest"] = serde_json::Value::String(recorded.to_string());
        stored["messages"]["entries"][0]["active_child"] = serde_json::Value::String(recorded.to_string());
        stored["messages"]["head"] = serde_json::Value::String(recorded.to_string());
        let loaded: Transcript = serde_json::from_value(stored).unwrap();
        assert_eq!(loaded.digest(), Some(recorded));
        assert!(loaded.prefix(&recorded).is_some(), "and references to it resolve");
    }

    #[test]
    fn a_turn_given_up_is_a_marked_branch_with_its_messages() {
        let mut transcript = Transcript::new();
        transcript.push(make_message(id(1)));
        let before = transcript.digest();
        transcript.extend(vec![make_message(id(2)), make_message(id(3))]);
        let last = transcript.digest().unwrap();
        let given_up = GivenUp { reason: GivenUpReason::Failed { message: "boom".to_string() }, at: Utc::now(), turn_entries: 2 };
        assert!(transcript.mark_given_up(&last, given_up.clone()));
        assert_eq!(transcript.digest(), Some(last), "the mark is no part of the digest");
        transcript.move_head(before.as_ref());
        transcript.push(make_message(id(4)));

        let loaded: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        let turns = loaded.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].given_up, given_up);
        assert_eq!(turns[0].messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(2), id(3)], "the turn's, not what came before");
        assert_eq!(loaded.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(4)]);
        assert!(!transcript.mark_given_up(&crate::digest::prompt("elsewhere"), given_up), "no such entry");
    }
}
