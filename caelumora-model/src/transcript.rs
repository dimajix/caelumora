//! An agent's transcript (issue 50): the chain of messages its context is
//! built from, their modifications and revocations (issue 234) and how each
//! of its turns ended (issues 145, 258), where a compaction's part of a
//! branch ends (issue 387) and who joined or left its channel (issue 408), as
//! what happened (issue 197).

use crate::chain::{Chain, Node};
use crate::actor::ActorId;
use crate::history::HistoryEntry;
use crate::confidentiality::Classification;
use crate::context::KnowledgeSource;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::digest;

/// What happened in a `Transcript`; its node carries the digest of the
/// transcript up to and including it (issue 06).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEntry {
    /// A message the model was given, or produced.
    Message(Message),
    /// A message the agent was given to read (issue 16) - not said by it,
    /// not heard from another -, and what it is, if it is knowledge: a
    /// skill loaded for it (issue 22), say; the summary of a compaction is
    /// none (issue 389). Given to the model as the message is; `source`
    /// says what it was given, so that what the agent knows can be told
    /// from the transcript - a compaction gives a skill again. `source` is
    /// not covered by the digest: the model is given the message alone.
    Given { message: Message, source: Option<KnowledgeSource> },
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
    /// The turn `turn_id` ended at `at` and was taken over (issue 258):
    /// its messages are those before it that carry its id.
    TurnFinished { turn_id: TurnId, at: DateTime<Utc> },
    /// The turn `turn_id` was given up at `at`, for `reason` (issues 145,
    /// 258): its messages, before it, stay where they are - what it said
    /// is not unsaid.
    TurnGivenUp { turn_id: TurnId, reason: GivenUpReason, at: DateTime<Utc> },
    /// The context was compacted at `at` (issues 16, 387): what comes
    /// before it on its branch - the replacement and the entries kept - was
    /// written by the compaction, what comes after was added where it
    /// stands. A message before it carries the measure of a call made on
    /// another branch (`Message::prompt_tokens`). Gives the model nothing.
    Compacted { at: DateTime<Utc> },
    /// `participant` joined the channel, as the channel's history records
    /// it (`HistoryEntry::ParticipantJoined`, issue 408), noted by the agent
    /// where it was told of it. No message: the model is told of it in the
    /// words its `ContextResolver` gives it.
    ParticipantJoined { participant: ActorId, name: String, by: ActorId, by_name: String, at: DateTime<Utc> },
    /// `participant` left the channel, as `ParticipantJoined` says.
    ParticipantLeft { participant: ActorId, name: String, by: ActorId, by_name: String, at: DateTime<Utc> },
}

/// An agent's transcript (issue 50): the sequence of messages its context is
/// built from - its own messages in full, with tool calls and internal
/// steps, and what others said as it received them - and how each of its
/// turns ended - the turn under way's messages too, as they are said (issue
/// 260). The context of a model call adds the prompt parts to it; the
/// history is the channel's public record,
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

/// The message `id` as `nodes` - a transcript's, in the order they were
/// added: of every branch, or of one path - hold it: as it was first
/// recorded, with every change of it since, in the order they were, and
/// nothing else cloned. `None` if they do not hold it. On every branch,
/// what a receiver given a branch holding it knows of it (issue 236), read
/// by a store, which holds every branch (`TranscriptStore::message`, issue
/// 380); on the active path, what a change of it is checked against
/// (`Transcript::standing`).
pub fn known<'a>(mut nodes: impl Iterator<Item = &'a Node<TranscriptEntry>>, id: MessageId) -> Option<Message> {
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

    /// Begins the active path anew as a compaction does (issue 16): a branch
    /// of its own from the root, with `replacement` - what stands for the
    /// older entries - given to read, each as its source if it is knowledge
    /// (issue 389), after it the entries of the active path from `keep_from`
    /// on - `None`: none - as they stand, and after them where the
    /// compaction's part of the branch ends (`TranscriptEntry::Compacted`,
    /// issue 387).
    ///
    /// Nothing is removed: the old branch stays, and every digest it had
    /// names what it did. Of the entries kept, a message is copied as it
    /// stands - a change of it is in it, one taken back is left out - and a
    /// turn's end as it is, an earlier compaction's end not at all
    /// (`TranscriptEntry::copy_standing`); every copy gets a digest of its
    /// own, since what comes before it is another.
    ///
    /// Returns where each entry of the old path stands on the new branch: one
    /// replaced at the end of the replacement - `None` without one -, one
    /// kept at its copy, or at the copy before it if it has none - a change
    /// of a message, which is in its copy. `None`, and nothing changes, if
    /// the active path is empty, if `keep_from` is not on it, or if the new
    /// branch would be empty.
    pub fn compact(&mut self, replacement: Vec<(Message, Option<KnowledgeSource>)>, keep_from: Option<Digest>) -> Option<HashMap<Digest, Option<Digest>>> {
        let path: Vec<_> = self.entries().collect();
        if path.is_empty() {
            return None;
        }
        let start = match keep_from {
            None => path.len(),
            Some(first) => path.iter().position(|node| node.digest == first)?,
        };
        let replaced: Vec<Digest> = path[..start].iter().map(|node| node.digest).collect();
        // Each entry kept with its copy as it stands, `None` if none is made.
        let kept: Vec<(Digest, Option<TranscriptEntry>)> = as_they_stand(path[start..].iter().copied())
            .map(|(node, standing)| (node.digest, node.entry.copy_standing(standing)))
            .collect();
        if replacement.is_empty() && kept.iter().all(|(_, copy)| copy.is_none()) {
            return None;
        }
        self.start_anew();
        for (message, source) in replacement {
            self.give(message, source);
        }
        let replacement_end = self.digest();
        let mut moved: HashMap<Digest, Option<Digest>> = replaced.into_iter().map(|old| (old, replacement_end)).collect();
        for (old, copy) in kept {
            if let Some(copy) = copy {
                self.record(copy);
            }
            moved.insert(old, self.digest());
        }
        self.record(TranscriptEntry::Compacted { at: Utc::now() });
        Some(moved)
    }

    /// Appends `msgs`.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m);
        }
    }

    /// Appends `msg` and returns the digest of the transcript before it.
    pub fn push(&mut self, msg: Message) -> Option<Digest> {
        self.record(TranscriptEntry::Message(msg))
    }

    /// Appends `msg`, given to read - as `source`, if it is knowledge
    /// (issues 16, 389) -, and returns the digest of the transcript before
    /// it.
    pub fn give(&mut self, msg: Message, source: Option<KnowledgeSource>) -> Option<Digest> {
        self.record(TranscriptEntry::Given { message: msg, source })
    }

    /// Records `entry`, what happened that is no change of a message
    /// (`TranscriptEntry::changed`): a message said or given - as `push`
    /// and `give` record one -, how a turn ended (issue 258), after its
    /// messages, where a compaction's part of a branch ends (issue 387),
    /// after what it wrote; a copy of any of them on a branch of its own
    /// (issue 16). A change is recorded by its own method - `modify`,
    /// `revoke`, `reclassify` -, which says whether it applies. Returns the
    /// digest of the transcript before it.
    pub fn record(&mut self, entry: TranscriptEntry) -> Option<Digest> {
        debug_assert!(entry.changed().is_none(), "a change is recorded by its own method");
        self.append(entry)
    }

    /// Appends `entry`, whatever it is, with its digest, and returns the
    /// digest of the transcript before it.
    fn append(&mut self, entry: TranscriptEntry) -> Option<Digest> {
        self.push_with(entry, digest::transcript_chain)
    }

    /// Records that the message `modified.id` of the active path is given
    /// as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if the message is not on the active path as it stands -
    /// never given to the model, on a branch no longer active, or taken
    /// back - or cannot be modified so (`Message::modify`). Taken in between
    /// turns - a change waits for a turn's end even where what is said is
    /// heard during it (issue 431) -: a turn under way produces its next
    /// message from the transcript as its context names it.
    pub fn modify(&mut self, modified: Message) -> bool {
        if !self.takes(&modified) {
            return false;
        }
        self.append(TranscriptEntry::Modified { message: modified });
        true
    }

    /// Whether `modify` takes `modified`: a change of a message of the
    /// active path as it stands that it can be modified to.
    pub fn takes(&self, modified: &Message) -> bool {
        self.standing(modified.id).is_some_and(|mut message| message.modify(modified))
    }

    /// Records that the message `id` of the active path was taken back at
    /// `at` (issue 234), so that the model is no longer given it. `false`,
    /// and nothing is recorded, if it is not on the active path as it
    /// stands, as for `modify`.
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        if self.standing(id).is_none() {
            return false;
        }
        self.append(TranscriptEntry::Revoked { message: id, at });
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
        self.append(TranscriptEntry::Reclassified { message: id, classification });
        true
    }

    /// The message `id` of the active path as it stands, `None` if it is
    /// not on it or was taken back.
    fn standing(&self, id: MessageId) -> Option<Message> {
        known(self.entries(), id).filter(|message| message.revoked.is_none())
    }
}

impl TranscriptEntry {
    /// What the transcript records of `entry` of the channel's history that
    /// the agent is told of rather than hears (issue 408): who joined or
    /// left. `None` for any other entry.
    pub fn of_announced(entry: &HistoryEntry) -> Option<Self> {
        match entry {
            HistoryEntry::ParticipantJoined { participant, name, by, by_name, at } => {
                Some(TranscriptEntry::ParticipantJoined { participant: *participant, name: name.clone(), by: *by, by_name: by_name.clone(), at: *at })
            }
            HistoryEntry::ParticipantLeft { participant, name, by, by_name, at, .. } => {
                Some(TranscriptEntry::ParticipantLeft { participant: *participant, name: name.clone(), by: *by, by_name: by_name.clone(), at: *at })
            }
            _ => None,
        }
    }

    /// The message, if this is one.
    pub fn message(&self) -> Option<&Message> {
        match self {
            TranscriptEntry::Message(message) | TranscriptEntry::Given { message, .. } => Some(message),
            TranscriptEntry::Modified { .. } | TranscriptEntry::Revoked { .. } | TranscriptEntry::Reclassified { .. }
            | TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. } | TranscriptEntry::Compacted { .. }
            | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. } => None,
        }
    }

    /// The copy of the entry a compaction keeps on a branch of its own
    /// (issue 16), given `standing`, its message as it stands on its path
    /// (`as_they_stand`): a message as it stands - a change of it is in the
    /// copy -, none if it was taken back; a turn's end and a change of who
    /// takes part as they are. Nothing of
    /// a change, which is in the copy of its message, nor of an earlier
    /// compaction's end, which ends what that compaction wrote.
    fn copy_standing(&self, standing: Option<Message>) -> Option<TranscriptEntry> {
        match (self, standing) {
            (TranscriptEntry::Message(_), Some(message)) => Some(TranscriptEntry::Message(message)),
            (TranscriptEntry::Given { source, .. }, Some(message)) => Some(TranscriptEntry::Given { message, source: *source }),
            (TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. } | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. }, _) => Some(self.clone()),
            (TranscriptEntry::Message(_) | TranscriptEntry::Given { .. }, None)
            | (TranscriptEntry::Modified { .. } | TranscriptEntry::Revoked { .. } | TranscriptEntry::Reclassified { .. } | TranscriptEntry::Compacted { .. }, _) => None,
        }
    }

    /// The turn whose end this is, if it is one.
    pub fn ended_turn(&self) -> Option<TurnId> {
        match self {
            TranscriptEntry::TurnFinished { turn_id, .. } | TranscriptEntry::TurnGivenUp { turn_id, .. } => Some(*turn_id),
            TranscriptEntry::Message(_) | TranscriptEntry::Given { .. } | TranscriptEntry::Modified { .. } | TranscriptEntry::Revoked { .. } | TranscriptEntry::Reclassified { .. }
            | TranscriptEntry::Compacted { .. } | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. } => None,
        }
    }

    /// How what it records of a message is classified: a message's, a
    /// modification's or a reclassification's classification. Who was
    /// given every entry has seen all of them (issue 236): they only add.
    pub fn classification(&self) -> Option<&Classification> {
        match self {
            TranscriptEntry::Message(message) | TranscriptEntry::Given { message, .. } | TranscriptEntry::Modified { message } => Some(&message.classification),
            TranscriptEntry::Reclassified { classification, .. } => Some(classification),
            TranscriptEntry::Revoked { .. } | TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. } | TranscriptEntry::Compacted { .. }
            | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. } => None,
        }
    }

    /// The message it modifies, takes back or reclassifies, if it does.
    pub fn changed(&self) -> Option<MessageId> {
        match self {
            TranscriptEntry::Modified { message } => Some(message.id),
            TranscriptEntry::Revoked { message, .. } | TranscriptEntry::Reclassified { message, .. } => Some(*message),
            TranscriptEntry::Message(_) | TranscriptEntry::Given { .. } | TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. }
            | TranscriptEntry::Compacted { .. } | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. } => None,
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
            TranscriptEntry::Message(_) | TranscriptEntry::Given { .. } | TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. }
            | TranscriptEntry::Compacted { .. } | TranscriptEntry::ParticipantJoined { .. } | TranscriptEntry::ParticipantLeft { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::{self, message_chain};
    use crate::workspace::{Workspace, WorkspaceId};
    use crate::message::testing::{id, make_message};
    use chrono::Duration;
    use crate::confidentiality::Confidentiality;
    use serde_json::Value;
    use std::slice;

    #[test]
    fn a_transcript_started_anew_holds_what_comes_after_and_keeps_what_it_held_as_a_branch() {
        let mut transcript = Transcript::default();
        let ids = |entries: Vec<&Node<TranscriptEntry>>| entries.into_iter().filter_map(|e| e.entry.message()).map(|m| m.id).collect::<Vec<_>>();
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
    fn a_message_given_is_read_as_its_message_and_hashed_as_one() {
        let source = KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), Some(digest::prompt("steps")));
        let (mut given, mut said) = (Transcript::default(), Transcript::default());
        given.push(make_message(id(1)));
        said.push(make_message(id(1)));
        given.give(make_message(id(2)), Some(source));
        said.push(make_message(id(2)));
        assert_eq!(given.digest(), said.digest(), "the model is given the same");
        assert_eq!(given.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(2)]);
        assert_eq!(known(given.all_entries(), id(2)).map(|m| m.id), Some(id(2)));
        let entry = &given.last().unwrap().entry;
        assert!(matches!(entry, TranscriptEntry::Given { source: Some(s), .. } if *s == source), "and keeps what it is");
        assert_eq!(entry.classification(), Some(&make_message(id(2)).classification));
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
        assert!(context.prefix(&digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn digests_are_kept_as_recorded() {
        let mut context = Transcript::default();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);

        // What was recorded stands, even if today's scheme would compute
        // something else - as it will for anything recorded under an
        // earlier version of it.
        let recorded = digest::prompt("recorded under another scheme");
        let mut stored = serde_json::to_value(&context).unwrap();
        stored["entries"][1]["digest"] = Value::String(recorded.to_string());
        stored["entries"][0]["active_child"] = Value::String(recorded.to_string());
        stored["head"] = Value::String(recorded.to_string());
        let loaded: Transcript = serde_json::from_value(stored).unwrap();
        assert_eq!(loaded.digest(), Some(recorded));
        assert!(loaded.prefix(&recorded).is_some(), "and references to it resolve");
    }

    /// The message `n` of the turn `turn`.
    fn of_turn(turn: TurnId, n: u128) -> Message {
        Message { turn_id: Some(turn), ..make_message(id(n)) }
    }

    /// The end of the turn `turn`, given up for `reason`, at `at`.
    fn given_up(turn: TurnId, reason: GivenUpReason, at: DateTime<Utc>) -> TranscriptEntry {
        TranscriptEntry::TurnGivenUp { turn_id: turn, reason, at }
    }

    /// A transcript of one message, a turn of two given up, and a message
    /// after: `(transcript, the turn)`.
    fn with_a_turn_given_up() -> (Transcript, TurnId) {
        let turn = TurnId::new();
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        transcript.extend(vec![of_turn(turn, 2), of_turn(turn, 3)]);
        transcript.record(given_up(turn, GivenUpReason::Failed { message: "boom".to_string() }, Utc::now()));
        transcript.push(make_message(id(4)));
        (transcript, turn)
    }

    /// How the turn `turn` ended, as the entries of `transcript` on its
    /// active path say.
    fn end_of(transcript: &Transcript, turn: TurnId) -> Option<TranscriptEntry> {
        transcript.entries().map(|n| n.entry.clone()).find(|e| e.ended_turn() == Some(turn))
    }

    #[test]
    fn a_turn_given_up_keeps_what_it_said_and_is_recorded_after_it() {
        let (transcript, turn) = with_a_turn_given_up();
        let loaded: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        assert!(matches!(end_of(&loaded, turn), Some(TranscriptEntry::TurnGivenUp { reason: GivenUpReason::Failed { .. }, .. })));
        assert_eq!(loaded.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(2), id(3), id(4)], "what it said is not unsaid");
        let entries: Vec<_> = loaded.entries().map(|n| n.entry.ended_turn().is_some()).collect();
        assert_eq!(entries, [false, false, false, true, false], "its end on the active path, after its messages");
        assert_eq!(loaded.all_entries().count(), 5, "and nothing branches");
    }

    #[test]
    fn the_end_of_a_turn_is_covered_by_the_digest_but_not_its_time() {
        let (transcript, turn) = with_a_turn_given_up();
        let ended = |end: TranscriptEntry| {
            let mut other = Transcript::default();
            other.push(make_message(id(1)));
            other.extend(vec![of_turn(turn, 2), of_turn(turn, 3)]);
            other.record(end);
            other.push(make_message(id(4)));
            other.digest()
        };
        let boom = GivenUpReason::Failed { message: "boom".to_string() };
        let earlier = Utc::now() - Duration::hours(1);
        assert_eq!(ended(given_up(turn, boom.clone(), earlier)), transcript.digest(), "the same end at another time");
        assert_ne!(ended(given_up(turn, GivenUpReason::Left, earlier)), transcript.digest(), "another reason");
        assert_ne!(ended(given_up(TurnId::new(), boom, earlier)), transcript.digest(), "another turn");
        assert_ne!(ended(TranscriptEntry::TurnFinished { turn_id: turn, at: earlier }), transcript.digest(), "taken over");
    }

    #[test]
    fn a_turn_given_up_reaches_the_stored_transcript_as_changes() {
        let mut transcript = Transcript::default();
        transcript.push(make_message(id(1)));
        let mut stored: Transcript = serde_json::from_value(serde_json::to_value(&transcript).unwrap()).unwrap();
        transcript.mark_stored();

        let turn = TurnId::new();
        transcript.extend(vec![of_turn(turn, 2), of_turn(turn, 3)]);
        transcript.record(given_up(turn, GivenUpReason::Left, Utc::now()));
        transcript.push(make_message(id(4)));

        stored.apply(transcript.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), transcript.digest());
        assert_eq!(stored.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(2), id(3), id(4)]);
        assert!(matches!(end_of(&stored, turn), Some(TranscriptEntry::TurnGivenUp { .. })), "the end came along");
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
        assert_eq!(classifications(transcript.messages()), slice::from_ref(&public), "as it stands");
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

        assert_eq!(known(transcript.all_entries(), id(1)).and_then(|m| m.content).as_deref(), Some("corrected"), "off the active path, with its change");
        assert!(known(transcript.all_entries(), id(2)).is_some_and(|m| m.revoked.is_some()), "taken back");
        assert_eq!(known(transcript.all_entries(), id(3)).map(|m| m.id), Some(id(3)), "on the active path");
        assert!(known(transcript.all_entries(), id(9)).is_none(), "never held");
    }

    #[test]
    fn what_an_entry_records_of_a_message_says_how_it_is_classified() {
        let confidential = Classification::general(Confidentiality::Confidential);
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

    /// Issues 16, 387, 389: a compaction writes a branch of its own - the
    /// replacement, then what it keeps as it stands, then its end - and says
    /// where every entry of the old path stands on it.
    #[test]
    fn a_compaction_begins_a_branch_of_its_own_and_says_where_the_old_entries_stand_on_it() {
        let mut transcript = Transcript::default();
        assert_eq!(transcript.compact(vec![(make_message(id(9)), None)], None), None, "not an empty transcript");
        let turn = TurnId::new();
        transcript.push(make_message(id(1)));
        transcript.push(make_message(id(2)));
        transcript.record(TranscriptEntry::TurnFinished { turn_id: turn, at: Utc::now() });
        transcript.extend(vec![make_message(id(3)), make_message(id(4))]);
        transcript.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) });
        transcript.revoke(id(4), Utc::now());
        transcript.record(TranscriptEntry::Compacted { at: Utc::now() });
        let old: Vec<Digest> = transcript.entries().map(|n| n.digest).collect();
        let head = transcript.digest();

        assert_eq!(transcript.compact(Vec::new(), None), None, "not to nothing");
        assert_eq!(transcript.compact(Vec::new(), old.last().copied()), None, "nor to an earlier compaction's end alone");
        assert_eq!(transcript.compact(vec![(make_message(id(9)), None)], Some(digest::prompt("elsewhere"))), None, "not from what is not on the active path");
        assert_eq!((transcript.digest(), transcript.all_entries().len()), (head, old.len()), "nothing changed");

        let source = KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), Some(digest::prompt("steps")));
        let moved = transcript.compact(vec![(make_message(id(10)), None), (make_message(id(11)), Some(source))], Some(old[1])).expect("compacted");

        let new: Vec<_> = transcript.entries().collect();
        let entries: Vec<(&str, Option<MessageId>)> = new.iter().map(|n| match &n.entry {
            TranscriptEntry::Message(m) => ("said", Some(m.id)),
            TranscriptEntry::Given { message, .. } => ("given", Some(message.id)),
            TranscriptEntry::TurnFinished { turn_id, .. } if *turn_id == turn => ("finished", None),
            TranscriptEntry::Compacted { .. } => ("compacted", None),
            _ => ("other", None),
        }).collect();
        assert_eq!(entries, [("given", Some(id(10))), ("given", Some(id(11))), ("said", Some(id(2))), ("finished", None), ("said", Some(id(3))), ("compacted", None)],
            "the replacement given, what was kept as it stands - the message taken back, the changes and the earlier end left out -, then its end");
        assert_eq!(new[4].entry.message().and_then(|m| m.content.as_deref()), Some("changed"), "a change is in its message's copy");
        let sources: Vec<_> = new[..2].iter().map(|n| match &n.entry { TranscriptEntry::Given { source, .. } => *source, _ => None }).collect();
        assert_eq!(sources, [None, Some(source)], "each as it was given, knowledge or not");
        assert!(transcript.prefix(&head.unwrap()).is_some(), "the old branch stays");

        let at = |i: usize| Some(new[i].digest);
        let stands: Vec<Option<Digest>> = old.iter().map(|digest| moved.get(digest).copied().flatten()).collect();
        assert_eq!(stands, [at(1), at(2), at(3), at(4), at(4), at(4), at(4), at(4)],
            "a replaced entry at the replacement's end, a kept one at its copy, or the copy before it");
        assert_eq!(moved.len(), old.len(), "every entry of the old path");
    }
}
