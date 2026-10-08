//! An agent's transcript (issue 50): the chain of messages its context is
//! built from, their modifications and revocations (issue 234) and the
//! turns it gave up (issue 145), as what happened (issue 197).

use crate::chain::{Chain, Node};
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What happened in a `Transcript`; its node carries the digest of the
/// transcript up to and including it (issue 06).
// Nearly every entry is a message: boxing it would cost an allocation per
// message and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEntry {
    /// A message the model was given, or produced.
    Message(Message),
    /// The message `message.id`, before it on the path, is given as
    /// `message` from here on (issue 234, `Message::modify`).
    Modified { message: Message },
    /// The message `message`, before it on the path, was taken back at `at`
    /// (issue 234, `Message::revoke`), and is no longer given.
    Revoked { message: MessageId, at: DateTime<Utc> },
    /// The message `message`, before it on the path, is classified
    /// `classification` from here on (issues 133, 239,
    /// `Message::reclassify`).
    Reclassified { message: MessageId, classification: Classification },
    /// A turn was given up (issue 145): its messages stay as a branch no
    /// longer active, ending in the entry `end` names, and the transcript
    /// goes on from here without them.
    GivenUp { end: Digest, given_up: GivenUp },
}

/// That a turn was given up rather than taken over (issue 145).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GivenUp {
    pub reason: GivenUpReason,
    pub at: DateTime<Utc>,
    /// The turn given up (issue 241): its messages, at the end of its
    /// branch, carry its id.
    pub turn_id: TurnId,
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
    Lowered { channel: Classification },
    /// The agent was stopped, leaving the channel: removed from it, or the
    /// channel deleted. Who did that is a matter of that act, not of its
    /// consequence - the audit log's (issue 140).
    Stopped,
    /// Its answer was asked for again while it was under way (issue 235):
    /// a turn of its own answers instead.
    Retried,
}

/// A turn that was given up: the record, and the turn's messages.
#[derive(Debug, Clone)]
pub struct GivenUpTurn {
    pub given_up: GivenUp,
    pub messages: Vec<Message>,
    /// The last message before the turn that is not internal - so one the
    /// agent's history holds as well: where the turn branched off what the
    /// agent heard and said (issue 146). `None` if there was none.
    pub after: Option<MessageId>,
}

/// An agent's transcript (issue 50): the sequence of messages its context is
/// built from - its own messages in full, with tool calls and internal
/// steps, and what others said as it received them - and the record of the
/// turns it gave up. The context of a model call adds the prompt parts and
/// the turn's messages to it; the history is the channel's public record,
/// which the transcript is not.
///
/// A chain: every entry's digest covers it and the digest before it, so
/// one digest names the whole transcript up to that point by content (see
/// `crate::chain`). Nothing is ever removed from it - also not from a branch
/// no longer active (issue 08) - so every digest it had names a prefix of
/// one of its branches, which can be recovered (`prefix`).
pub type Transcript = Chain<TranscriptEntry>;

/// The entries of `path` - a path of a transcript from its root, as
/// `entries` or `prefix` give it - each with the message it holds as it
/// stands at the end of the path, `None` if it holds none: what a model
/// given the path is given, and where. Every modification and revocation
/// on the path applies to the message it names, which comes before it, so
/// the path is gone through twice: once for them, then for the messages. A
/// message taken back
/// holds none any more; its entry stays, and a prompt part placed after it
/// stays where it was. The one rule every reader of a transcript's messages
/// reads them by (issue 234); what has to consider every entry ever added -
/// the tokens used, whether a message reached the transcript at all - reads
/// `all_entries` instead.
pub fn as_they_stand<'a, P>(path: P) -> impl Iterator<Item = (&'a Node<TranscriptEntry>, Option<Message>)>
where
    P: IntoIterator<Item = &'a Node<TranscriptEntry>>,
    P::IntoIter: Clone,
{
    let path = path.into_iter();
    let mut changes: HashMap<MessageId, Vec<&'a TranscriptEntry>> = HashMap::new();
    for node in path.clone() {
        if let Some(id) = node.entry.changed() {
            changes.entry(id).or_default().push(&node.entry);
        }
    }
    path.map(move |node| {
        let message = node.entry.message().and_then(|message| {
            let mut message = message.clone();
            for change in changes.get(&message.id).into_iter().flatten() {
                change.apply_to(&mut message);
            }
            message.revoked.is_none().then_some(message)
        });
        (node, message)
    })
}

/// The message `id` as `nodes` hold it: as it was first recorded, with
/// every change of it since, in the order they were. `None` if they do not
/// hold it.
fn known<'a>(mut nodes: impl Iterator<Item = &'a Node<TranscriptEntry>>, id: MessageId) -> Option<Message> {
    let mut message = nodes.by_ref().find_map(|node| node.entry.message().filter(|m| m.id == id)).cloned()?;
    nodes.filter(|node| node.entry.changed() == Some(id)).for_each(|node| node.entry.apply_to(&mut message));
    Some(message)
}

impl Chain<TranscriptEntry> {
    /// The messages of the active path as they stand: what the model is
    /// given.
    pub fn messages(&self) -> Vec<Message> {
        as_they_stand(self.entries()).filter_map(|(_, message)| message).collect()
    }

    /// Starts the active path anew, and returns the digest of what it held
    /// (issue 205): for a receiver that keeps what it was given - a remote
    /// agent - once the active path was given to it. What was given stays
    /// as a branch from the root, which the digest names, and what comes
    /// after is what the receiver is given next.
    pub fn start_anew(&mut self) -> Option<Digest> {
        let given = self.digest();
        self.move_head(None);
        given
    }

    /// Appends `msgs`.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m);
        }
    }

    /// Appends `msg` and returns the digest of the transcript before it.
    pub fn push(&mut self, msg: Message) -> Option<Digest> {
        self.push_with(TranscriptEntry::Message(msg), crate::digest::transcript_chain)
    }

    /// Records that the message `modified.id` of the active path is given
    /// as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if the message is not on the active path as it stands -
    /// never given to the model, on a branch no longer active, or taken
    /// back - or cannot be modified so (`Message::modify`). Not while a
    /// turn is under way, as for every entry: its messages continue the
    /// transcript as it was when it began.
    pub fn modify(&mut self, modified: Message) -> bool {
        if !self.standing(modified.id).is_some_and(|mut m| m.modify(&modified)) {
            return false;
        }
        self.push_with(TranscriptEntry::Modified { message: modified }, crate::digest::transcript_chain);
        true
    }

    /// Records that the message `id` of the active path was taken back at
    /// `at` (issue 234), so that the model is no longer given it. `false`,
    /// and nothing is recorded, if it is not on the active path as it
    /// stands, as for `modify`.
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        if self.standing(id).is_none() {
            return false;
        }
        self.push_with(TranscriptEntry::Revoked { message: id, at }, crate::digest::transcript_chain);
        true
    }

    /// Records that the message `id` of the active path is classified
    /// `classification` from now on (issue 239): what a turn begun
    /// afterwards has seen of it (issue 168). `false`, and nothing is
    /// recorded, if it is not on the active path as it stands, as for
    /// `modify`.
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        if self.standing(id).is_none() {
            return false;
        }
        self.push_with(TranscriptEntry::Reclassified { message: id, classification }, crate::digest::transcript_chain);
        true
    }

    /// The message `id` as the transcript holds it, on whichever branch:
    /// as it was first recorded, with every change of it recorded since.
    /// `None` if it holds none. What a receiver given a branch holding it
    /// knows of it (issue 236), where `messages` gives only the active path.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        known(self.all_entries(), id)
    }

    /// The message `id` of the active path as it stands, `None` if it is
    /// not on it or was taken back.
    fn standing(&self, id: MessageId) -> Option<Message> {
        as_they_stand(self.entries()).find_map(|(_, m)| m.filter(|m| m.id == id))
    }

    /// Records that the turn whose branch ends in the entry `end` names was
    /// given up (issue 145), here - after the head went back to where the
    /// turn began. `false`, and nothing is recorded, if there is no such
    /// entry.
    pub fn give_up(&mut self, end: Digest, given_up: GivenUp) -> bool {
        if self.prefix(&end).is_none() {
            return false;
        }
        self.push_with(TranscriptEntry::GivenUp { end, given_up }, crate::digest::transcript_chain);
        true
    }

    /// The turns given up, in the order they were, each with its messages.
    pub fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        self.all_entries()
            .filter_map(|node| {
                let TranscriptEntry::GivenUp { end, given_up } = &node.entry else { return None };
                let path: Vec<_> = as_they_stand(self.prefix(end)?).collect();
                let of_turn = |message: &Option<Message>| message.as_ref().is_some_and(|m| m.turn_id == Some(given_up.turn_id));
                let start = path.iter().rposition(|(_, message)| !of_turn(message)).map_or(0, |last| last + 1);
                let (before, turn) = path.split_at(start);
                let messages = turn.iter().filter_map(|(_, message)| message.clone()).collect();
                let after = before.iter().rev().filter_map(|(_, message)| message.as_ref()).find(|m| !m.internal && m.notice_of.is_none()).map(|m| m.id);
                Some(GivenUpTurn { given_up: given_up.clone(), messages, after })
            })
            .collect()
    }
}

impl TranscriptEntry {
    /// The message, if this is one.
    pub fn message(&self) -> Option<&Message> {
        match self {
            TranscriptEntry::Message(message) => Some(message),
            TranscriptEntry::Modified { .. } | TranscriptEntry::Revoked { .. } | TranscriptEntry::Reclassified { .. } | TranscriptEntry::GivenUp { .. } => None,
        }
    }

    /// How what it records of a message is classified: a message's, a
    /// modification's or a reclassification's classification. Who was
    /// given every entry has seen all of them (issue 236): they only add.
    pub fn classification(&self) -> Option<&Classification> {
        match self {
            TranscriptEntry::Message(message) | TranscriptEntry::Modified { message } => Some(&message.classification),
            TranscriptEntry::Reclassified { classification, .. } => Some(classification),
            TranscriptEntry::Revoked { .. } | TranscriptEntry::GivenUp { .. } => None,
        }
    }

    /// The message it modifies, takes back or reclassifies, if it does.
    fn changed(&self) -> Option<MessageId> {
        match self {
            TranscriptEntry::Modified { message } => Some(message.id),
            TranscriptEntry::Revoked { message, .. } | TranscriptEntry::Reclassified { message, .. } => Some(*message),
            TranscriptEntry::Message(_) | TranscriptEntry::GivenUp { .. } => None,
        }
    }

    /// Applies what it changes to `message` as it stands, if it changes it.
    fn apply_to(&self, message: &mut Message) {
        match self {
            TranscriptEntry::Modified { message: modified } => {
                message.modify(modified);
            }
            TranscriptEntry::Revoked { at, .. } => {
                message.revoke(*at);
            }
            TranscriptEntry::Reclassified { classification, .. } => {
                message.reclassify(classification);
            }
            TranscriptEntry::Message(_) | TranscriptEntry::GivenUp { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::message_chain;
    use crate::message::tests::{id, make_message};

    #[test]
    fn a_transcript_started_anew_holds_what_comes_after_and_keeps_what_it_held_as_a_branch() {
        let mut transcript = Transcript::default();
        let ids = |entries: Vec<&crate::chain::Node<TranscriptEntry>>| entries.into_iter().filter_map(|e| e.entry.message()).map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(transcript.start_anew(), None, "it held nothing");
        transcript.extend(vec![make_message(id(1)), make_message(id(2))]);
        let first = transcript.start_anew().expect("what it held");
        assert!(transcript.messages().is_empty(), "nothing new yet");
        transcript.extend(vec![make_message(id(3))]);
        let second = transcript.start_anew().expect("what it held since");
        assert_eq!(ids(transcript.prefix(&first).unwrap()), vec![id(1), id(2)]);
        assert_eq!(ids(transcript.prefix(&second).unwrap()), vec![id(3)], "only what came after the first");
        assert_eq!(ids(transcript.all_entries().collect()), vec![id(1), id(2), id(3)], "all it ever held, in order");
    }

    #[test]
    fn message_context_empty_state() {
        let context = Transcript::default();

        assert_eq!(context.digest(), None);
        assert!(context.messages().is_empty());
    }

    #[test]
    fn message_context_chains_the_digests_of_its_messages() {
        let (m1, m2, m3) = (make_message(id(1)), make_message(id(2)), make_message(id(3)));

        let mut context = Transcript::default();
        let before: Vec<_> = [&m1, &m2, &m3].into_iter().map(|m| context.push(m.clone())).collect();

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

        let mut extend_context = Transcript::default();
        extend_context.extend(messages.clone());

        let mut push_context = Transcript::default();
        for message in messages {
            push_context.push(message);
        }

        assert_eq!(extend_context.digest(), push_context.digest());
    }

    #[test]
    fn a_digest_names_the_prefix_it_was_taken_at() {
        let mut context = Transcript::default();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);
        let at_two = context.digest().unwrap();
        context.push(make_message(id(3)));

        let prefix = context.prefix(&at_two).expect("an earlier digest names a prefix");
        assert_eq!(prefix.iter().filter_map(|e| e.entry.message()).map(|m| m.id).collect::<Vec<_>>(), vec![id(1), id(2)]);
        assert!(context.prefix(&crate::digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn digests_are_kept_as_recorded() {
        let mut context = Transcript::default();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);

        // What was recorded stands, even if today's scheme would compute
        // something else - as it will for anything recorded under an
        // earlier version of it.
        let recorded = crate::digest::prompt("recorded under another scheme");
        let mut stored = serde_json::to_value(&context).unwrap();
        stored["entries"][1]["digest"] = serde_json::Value::String(recorded.to_string());
        stored["entries"][0]["active_child"] = serde_json::Value::String(recorded.to_string());
        stored["head"] = serde_json::Value::String(recorded.to_string());
        let loaded: Transcript = serde_json::from_value(stored).unwrap();
        assert_eq!(loaded.digest(), Some(recorded));
        assert!(loaded.prefix(&recorded).is_some(), "and references to it resolve");
    }

    /// The message `n` of the turn `turn`.
    fn of_turn(turn: TurnId, n: u128) -> Message {
        Message { turn_id: Some(turn), ..make_message(id(n)) }
    }

    /// A transcript of one message, a turn of two given up, and a message
    /// after: `(transcript, the record)`.
    fn with_a_turn_given_up() -> (Transcript, GivenUp) {
        let turn = TurnId::new();
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        let before = transcript.digest();
        transcript.extend(vec![of_turn(turn, 2), of_turn(turn, 3)]);
        let end = transcript.digest().unwrap();
        let given_up = GivenUp { reason: GivenUpReason::Failed { message: "boom".to_string() }, at: Utc::now(), turn_id: turn };
        transcript.move_head(before.as_ref());
        assert!(transcript.give_up(end, given_up.clone()));
        transcript.push(make_message(id(4)));
        (transcript, given_up)
    }

    #[test]
    fn a_turn_given_up_is_recorded_after_its_branch_with_its_messages() {
        let (mut transcript, given_up) = with_a_turn_given_up();
        let loaded: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        let turns = loaded.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].given_up, given_up);
        assert_eq!(turns[0].messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(2), id(3)], "the turn's, not what came before");
        assert!(!transcript.give_up(crate::digest::prompt("elsewhere"), given_up.clone()), "no such branch");
        assert_eq!(turns[0].after, Some(id(1)), "where it branched off");
        assert_eq!(loaded.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(4)], "the model is given the messages");
        assert_eq!(loaded.entries().len(), 3, "the record is on the active path");
    }

    #[test]
    fn a_turn_given_up_branched_off_after_the_last_message_that_is_not_internal() {
        let turn = TurnId::new();
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        // An internal step of an earlier turn.
        transcript.push(Message { internal: true, turn_id: Some(TurnId::new()), ..make_message(id(2)) });
        let before = transcript.digest();
        transcript.push(of_turn(turn, 3));
        let end = transcript.digest().unwrap();
        transcript.move_head(before.as_ref());
        transcript.give_up(end, GivenUp { reason: GivenUpReason::Stopped, at: Utc::now(), turn_id: turn });
        assert_eq!(transcript.given_up_turns()[0].messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(3)], "the turn's alone, not the earlier one's");
        assert_eq!(transcript.given_up_turns()[0].after, Some(id(1)), "an internal message is not in the history");

        let mut told = Transcript::default();
        told.push(make_message(id(1)));
        told.push(Message { notice_of: Some(id(7)), ..make_message(id(2)) });
        let before = told.digest();
        told.push(of_turn(turn, 3));
        let end = told.digest().unwrap();
        told.move_head(before.as_ref());
        told.give_up(end, GivenUp { reason: GivenUpReason::Stopped, at: Utc::now(), turn_id: turn });
        assert_eq!(told.given_up_turns()[0].after, Some(id(1)), "nor is a notice");

        let mut first = Transcript::default();
        first.push(of_turn(turn, 1));
        let end = first.digest().unwrap();
        first.move_head(None);
        first.give_up(end, GivenUp { reason: GivenUpReason::Stopped, at: Utc::now(), turn_id: turn });
        assert_eq!(first.given_up_turns()[0].after, None, "nothing before it");
    }

    #[test]
    fn the_record_of_a_turn_given_up_is_covered_by_the_digest_but_not_its_time() {
        let (transcript, given_up) = with_a_turn_given_up();
        let mut other = Transcript::default();
        other.push(make_message(id(1)));
        let before = other.digest();
        other.extend(vec![make_message(id(2)), make_message(id(3))]);
        let end = other.digest().unwrap();
        other.move_head(before.as_ref());
        other.give_up(end, GivenUp { at: Utc::now() - chrono::Duration::hours(1), ..given_up.clone() });
        other.push(make_message(id(4)));
        assert_eq!(other.digest(), transcript.digest(), "the same record at another time");

        other.move_head(before.as_ref());
        other.give_up(end, GivenUp { reason: GivenUpReason::Stopped, ..given_up.clone() });
        assert_ne!(other.digest(), transcript.digest(), "another reason");
        other.move_head(before.as_ref());
        other.give_up(end, GivenUp { turn_id: TurnId::new(), ..given_up });
        assert_ne!(other.digest(), transcript.digest(), "another turn");
    }

    #[test]
    fn a_turn_given_up_reaches_the_stored_transcript_as_changes() {
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        let mut stored: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        transcript.mark_stored();

        let turn = TurnId::new();
        let before = transcript.digest();
        transcript.extend(vec![of_turn(turn, 2), of_turn(turn, 3)]);
        let end = transcript.digest().unwrap();
        transcript.move_head(before.as_ref());
        transcript.give_up(end, GivenUp { reason: GivenUpReason::Stopped, at: Utc::now(), turn_id: turn });
        transcript.push(make_message(id(4)));

        stored.apply(transcript.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), transcript.digest());
        assert_eq!(stored.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(4)]);
        assert_eq!(stored.given_up_turns().len(), 1, "the record came along");
        assert_eq!(stored.given_up_turns()[0].messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(2), id(3)]);
    }

    #[test]
    fn a_change_is_given_to_the_model_from_where_it_was_recorded() {
        let at = Utc::now();
        let texts = |messages: Vec<Message>| messages.into_iter().map(|m| m.content.unwrap_or_default()).collect::<Vec<_>>();
        let mut transcript = Transcript::default();
        transcript.extend(vec![make_message(id(1)), Message { content: Some("second".to_string()), ..make_message(id(2)) }]);
        let before = transcript.digest().unwrap();

        assert!(transcript.modify(Message { content: Some("corrected".to_string()), timestamp: at, ..make_message(id(1)) }));
        assert!(transcript.revoke(id(2), at));
        assert!(!transcript.revoke(id(2), at), "taken back already");
        assert!(!transcript.modify(Message { content: Some("too late".to_string()), ..make_message(id(2)) }), "taken back already");
        assert!(!transcript.revoke(id(9), at), "never given");
        transcript.push(make_message(id(3)));

        assert_eq!(texts(transcript.messages()), ["corrected", "hello"], "as it stands, without what was taken back");
        let then: Vec<Message> = as_they_stand(transcript.prefix(&before).unwrap()).filter_map(|(_, m)| m).collect();
        assert_eq!(texts(then), ["hello", "second"], "a digest taken before names what was given then");
        let entries = as_they_stand(transcript.entries()).map(|(_, m)| m.map(|m| m.id)).collect::<Vec<_>>();
        assert_eq!(entries, [Some(id(1)), None, None, None, Some(id(3))], "every entry stays, in its place");

        let loaded: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        assert_eq!(texts(loaded.messages()), ["corrected", "hello"]);
        assert_eq!(loaded.digest(), transcript.digest());
    }

    #[test]
    fn a_reclassification_is_given_from_where_it_was_recorded_and_reaches_the_stored_transcript() {
        let public = Classification::PUBLIC;
        let classifications = |messages: Vec<Message>| messages.into_iter().map(|m| m.classification).collect::<Vec<_>>();
        let mut transcript = Transcript::default();
        transcript.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        transcript.mark_stored();
        let before = transcript.digest().unwrap();

        assert!(transcript.reclassify(id(1), public.clone()));
        assert_ne!(transcript.digest(), Some(before), "recorded: the digest covers it");
        transcript.revoke(id(2), Utc::now());
        assert!(!transcript.reclassify(id(2), public.clone()), "taken back");
        assert!(!transcript.reclassify(id(9), public.clone()), "never given");

        let internal = make_message(id(1)).classification;
        assert_eq!(classifications(transcript.messages()), std::slice::from_ref(&public), "as it stands");
        let then: Vec<Message> = as_they_stand(transcript.prefix(&before).unwrap()).filter_map(|(_, m)| m).collect();
        assert_eq!(classifications(then), [internal.clone(), internal], "a digest taken before names what was given then");

        stored.apply(transcript.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), transcript.digest());
        assert_eq!(classifications(stored.messages()), [public]);
    }

    #[test]
    fn a_message_is_found_on_whichever_branch_holds_it_as_it_stands_there() {
        let at = Utc::now();
        let mut transcript = Transcript::default();
        transcript.extend(vec![make_message(id(1)), make_message(id(2))]);
        assert!(transcript.modify(Message { content: Some("corrected".to_string()), timestamp: at, ..make_message(id(1)) }));
        transcript.revoke(id(2), at);
        transcript.start_anew();
        transcript.push(make_message(id(3)));

        assert_eq!(transcript.message(id(1)).and_then(|m| m.content).as_deref(), Some("corrected"), "off the active path, with its change");
        assert!(transcript.message(id(2)).is_some_and(|m| m.revoked.is_some()), "taken back");
        assert_eq!(transcript.message(id(3)).map(|m| m.id), Some(id(3)), "on the active path");
        assert!(transcript.message(id(9)).is_none(), "never held");
    }

    #[test]
    fn what_an_entry_records_of_a_message_says_how_it_is_classified() {
        let confidential = Classification::general(crate::confidentiality::Confidentiality::Confidential);
        let modified = TranscriptEntry::Modified { message: Message { classification: confidential.clone(), ..make_message(id(1)) } };
        assert_eq!(modified.classification(), Some(&confidential), "a modification may raise it");
        let released = TranscriptEntry::Reclassified { message: id(1), classification: Classification::PUBLIC };
        assert_eq!(released.classification(), Some(&Classification::PUBLIC));
        assert_eq!(TranscriptEntry::Revoked { message: id(1), at: Utc::now() }.classification(), None, "says nothing");
    }

    #[test]
    fn a_change_applies_only_on_the_active_path() {
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        let fork = transcript.digest();
        transcript.push(make_message(id(2)));
        transcript.move_head(fork.as_ref());
        assert!(!transcript.revoke(id(2), Utc::now()), "not given to the model any more");
        assert_eq!(transcript.all_entries().len(), 2, "nothing recorded");
    }
}
