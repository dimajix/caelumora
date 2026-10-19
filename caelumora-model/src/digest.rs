//! Content digests (issue 06): SHA-256 hashes that identify what an agent
//! saw - a chain of messages, a prompt - by their content.
//!
//! A `Digest` is written as 64 lowercase hex digits, in JSON as well, so that
//! stored state stays readable. What goes into a digest is defined here, in
//! one place, with a version tag in front of it, so that the scheme can
//! change without old and new digests ever being mistaken for each other.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::context::FrameRef;
use crate::history::{HistoryEntry, Recorded};
use crate::message::{Message, MessageId};
use crate::transcript::TranscriptEntry;
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;
use crate::ledger::{Change, LedgerKind};
use crate::tool::ToolSet;
use serde::de::Error;

/// A SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

impl Digest {
    /// An id derived from the digest, the same each time (issue 408): what
    /// the model is given for an entry of a transcript that is no message is
    /// named by its entry's digest.
    pub(crate) fn as_uuid(&self) -> Uuid {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&self.0[..16]);
        Uuid::from_bytes(bytes)
    }
}


impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({self})")
    }
}

/// A string that is not 64 hex digits.
#[derive(Debug, thiserror::Error)]
#[error("not a digest: expected 64 hex digits")]
pub struct InvalidDigest;

impl FromStr for Digest {
    type Err = InvalidDigest;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(InvalidDigest);
        }
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(InvalidDigest),
        };
        let mut out = [0u8; 32];
        for (i, pair) in bytes.chunks(2).enumerate() {
            out[i] = nibble(pair[0])? << 4 | nibble(pair[1])?;
        }
        Ok(Digest(out))
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(Error::custom)
    }
}

/// Feeds values into a hash unambiguously: every value is preceded by its
/// length, so that no two different sequences produce the same input.
pub struct Encoder(Sha256);

impl Encoder {
    /// An encoder of the scheme `scheme`, which names what is digested and
    /// how, so that two schemes never give one digest.
    pub fn new(scheme: &str) -> Self {
        let mut encoder = Encoder(Sha256::new());
        encoder.bytes(scheme.as_bytes());
        encoder
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }

    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn opt_str(&mut self, s: Option<&str>) {
        match s {
            None => self.0.update([0u8]),
            Some(s) => {
                self.0.update([1u8]);
                self.str(s);
            }
        }
    }

    fn opt_digest(&mut self, digest: Option<&Digest>) {
        match digest {
            None => self.0.update([0u8]),
            Some(d) => {
                self.0.update([1u8]);
                self.0.update(d.0);
            }
        }
    }

    /// A JSON value with the keys of every object sorted, so that the digest
    /// does not depend on the order in which a value was built or parsed.
    fn json(&mut self, value: &Value) {
        let mut out = String::new();
        write_canonical(value, &mut out);
        self.str(&out);
    }

    pub fn finish(self) -> Digest {
        Digest(self.0.finalize().into())
    }
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `value` as JSON, for hashing. The types hashed here derive `Serialize`
/// and have no maps with non-string keys, so conversion cannot fail; were it
/// to, hashing `Null` instead would give different contents the same digest,
/// which an integrity check must not do silently.
fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("hashed model types serialise to JSON")
}

/// The digest of a chain of messages ending in `message`, whose predecessor
/// chain has the digest `previous` (`None` for the first message).
///
/// It covers what the model sees of a message - id, role, sender, content,
/// reasoning, tool calls and results, whether it is internal - and not what
/// only describes it afterwards: timestamps, usage statistics, resolved
/// mentions and metadata. Its predecessor is covered as `previous`.
pub fn message_chain(previous: Option<&Digest>, message: &Message) -> Digest {
    let mut e = Encoder::new("caelumora.message-chain.v1");
    e.opt_digest(previous);
    e.bytes(message.id.0.as_bytes());
    e.json(&to_json(&message.role));
    e.json(&to_json(&message.sender_id));
    e.opt_str(message.content.as_deref());
    e.opt_str(message.reasoning.as_deref());
    e.json(&to_json(&message.tool_calls));
    e.json(&to_json(&message.tool_results));
    e.bytes(&[message.internal as u8]);
    e.finish()
}

/// The digest of a chain of messages ending in the modification of one of
/// them to `modified` (issue 234) - the same in a history and a transcript -
/// whose predecessor chain has the digest `previous`. It covers the message
/// and what the modification takes over that a model sees or that decides
/// where it may go - content, reasoning, classification - not when it was
/// made.
pub fn message_modified(previous: Option<&Digest>, modified: &Message) -> Digest {
    let mut e = Encoder::new("caelumora.message-modified.v1");
    e.opt_digest(previous);
    e.bytes(modified.id.0.as_bytes());
    e.opt_str(modified.content.as_deref());
    e.opt_str(modified.reasoning.as_deref());
    e.json(&to_json(&modified.classification));
    e.finish()
}

/// The digest of a chain of messages ending in `message` taken back (issue
/// 234), as `message_modified`: the message, not when.
pub fn message_revoked(previous: Option<&Digest>, message: MessageId) -> Digest {
    let mut e = Encoder::new("caelumora.message-revoked.v1");
    e.opt_digest(previous);
    e.bytes(message.0.as_bytes());
    e.finish()
}

/// The digest of a chain of messages ending in the reclassification of
/// `message` as `classification` (issue 133) - the same in a history and a
/// transcript (issue 239) - whose predecessor chain has the digest
/// `previous`: in the scheme the history's entry had first, so that every
/// digest recorded names what it named.
pub fn message_reclassified(previous: Option<&Digest>, message: MessageId, classification: &Classification) -> Digest {
    let mut e = Encoder::new("caelumora.history-reclassified.v1");
    e.opt_digest(previous);
    e.bytes(message.0.as_bytes());
    e.json(&to_json(classification));
    e.finish()
}

/// The digest of a transcript (issue 50) ending in `entry`, whose
/// predecessor chain has the digest `previous`: a message as
/// `message_chain`, so that every digest recorded before issue 197 names
/// what it named - one given to read (issue 16) alike, since the model is
/// given the same; a turn's end (issue 258) by what it says - the turn and,
/// given up, the reason, not the time - in schemes of their own, and where a
/// compaction's part ends (issue 387) by its place alone, a change of who
/// takes part (issue 408) as `taking_part`; a message
/// modified, taken back or reclassified as `message_modified`,
/// `message_revoked` and `message_reclassified`.
pub fn transcript_chain(previous: Option<&Digest>, entry: &TranscriptEntry) -> Digest {
    match entry {
        TranscriptEntry::Message(message) | TranscriptEntry::Given { message, .. } => message_chain(previous, message),
        TranscriptEntry::Modified { message } => message_modified(previous, message),
        TranscriptEntry::Revoked { message, .. } => message_revoked(previous, *message),
        TranscriptEntry::Reclassified { message, classification } => message_reclassified(previous, *message, classification),
        TranscriptEntry::TurnFinished { turn_id, .. } => turn_ended("caelumora.transcript-turn-finished.v1", previous, *turn_id, None, None),
        TranscriptEntry::TurnGivenUp { turn_id, reason, .. } => turn_ended("caelumora.transcript-turn-given-up.v1", previous, *turn_id, None, Some(reason)),
        TranscriptEntry::Compacted { .. } => {
            let mut e = Encoder::new("caelumora.transcript-compacted.v1");
            e.opt_digest(previous);
            e.finish()
        }
        TranscriptEntry::ParticipantJoined { participant, name, by, by_name, .. } => taking_part("caelumora.transcript-participant-joined.v1", previous, participant, name, by, by_name),
        TranscriptEntry::ParticipantLeft { participant, name, by, by_name, .. } => taking_part("caelumora.transcript-participant-left.v1", previous, participant, name, by, by_name),
    }
}

/// The digest of a chain ending in the end of the turn `turn` (issue 258),
/// in `scheme`: the turn, whose it was if the chain records it, and, if it
/// was given up, the `reason` - not the time.
fn turn_ended(scheme: &str, previous: Option<&Digest>, turn: TurnId, actor: Option<&ActorId>, reason: Option<&GivenUpReason>) -> Digest {
    let mut e = Encoder::new(scheme);
    e.opt_digest(previous);
    e.bytes(turn.0.as_bytes());
    if let Some(actor) = actor {
        e.json(&to_json(actor));
    }
    if let Some(reason) = reason {
        e.json(&to_json(reason));
    }
    e.finish()
}

/// The digest of a chain ending in someone joining or leaving (issue 408),
/// in `scheme`: who - `participant`, called `name` - and by whose act -
/// `by`, called `by_name` -, as the model is told of it; not when.
fn taking_part(scheme: &str, previous: Option<&Digest>, participant: &ActorId, name: &str, by: &ActorId, by_name: &str) -> Digest {
    let mut e = Encoder::new(scheme);
    e.opt_digest(previous);
    e.json(&to_json(participant));
    e.str(name);
    e.json(&to_json(by));
    e.str(by_name);
    e.finish()
}

/// The digest of a history (issue 06) ending in `recorded`, whose
/// predecessor chain has the digest `previous`: of what the entry says, who
/// sent it and whom it went to, as recorded (issue 417) - not the head of
/// the envelope it came in (issue 223), which says when and in which chain
/// of events, not what. What an entry says is digested
/// by its kind: a message said as `message_chain`; the channel's
/// classification fitted into (issue 133), a turn's end (issue 258) and the
/// start of a step of its plan (issue 263) in schemes of their own - the
/// step by what it says, not when -, a command carried out (issue 269), a
/// context compacted (issue 16) and a change of who takes part (issue 408)
/// likewise - by what they say, not when; a message modified, taken back or
/// reclassified as `message_modified`, `message_revoked` and
/// `message_reclassified`.
pub fn history_chain(previous: Option<&Digest>, recorded: &Recorded) -> Digest {
    let mut e = Encoder::new("caelumora.history-recorded.v3");
    e.opt_digest(previous);
    e.opt_digest(Some(&history_entry(None, &recorded.entry)));
    e.json(&to_json(&recorded.sender));
    e.json(&to_json(&recorded.recipients));
    e.finish()
}

/// The digest of `entry` after `previous`, by what it says: the part of a
/// history's digest (`history_chain`) that the entry itself contributes.
fn history_entry(previous: Option<&Digest>, entry: &HistoryEntry) -> Digest {
    match entry {
        HistoryEntry::Said { message } => message_chain(previous, message),
        HistoryEntry::Given { message } => {
            let mut e = Encoder::new("caelumora.history-given.v1");
            e.opt_digest(previous);
            e.opt_digest(Some(&message_chain(None, message)));
            e.finish()
        }
        HistoryEntry::Modified { message } => message_modified(previous, message),
        HistoryEntry::Revoked { message, .. } => message_revoked(previous, *message),
        HistoryEntry::Reclassified { message, classification } => message_reclassified(previous, *message, classification),
        HistoryEntry::Fitted { channel } => {
            let mut e = Encoder::new("caelumora.history-fitted.v1");
            e.opt_digest(previous);
            e.json(&to_json(channel));
            e.finish()
        }
        HistoryEntry::TurnFinished { turn_id, actor, .. } => turn_ended("caelumora.history-turn-finished.v1", previous, *turn_id, Some(actor), None),
        HistoryEntry::TurnGivenUp { turn_id, actor, reason, .. } => turn_ended("caelumora.history-turn-given-up.v1", previous, *turn_id, Some(actor), Some(reason)),
        HistoryEntry::PlanStepStarted { turn_id, actor, step_index, total_steps, description, .. } => {
            let mut e = Encoder::new("caelumora.history-plan-step-started.v1");
            e.opt_digest(previous);
            e.bytes(turn_id.0.as_bytes());
            e.json(&to_json(actor));
            e.json(&serde_json::json!([step_index, total_steps]));
            e.str(description);
            e.finish()
        }
        HistoryEntry::CommandExecuted { turn_id, actor, issuer, command, args, output, to, .. } => {
            let mut e = Encoder::new("caelumora.history-command-executed.v2");
            e.opt_digest(previous);
            e.json(&serde_json::json!([turn_id, actor, issuer, to]));
            e.str(command);
            e.str(args);
            e.str(output);
            e.finish()
        }
        HistoryEntry::CommandFailed { turn_id, actor, command, reason, to, .. } => {
            let mut e = Encoder::new("caelumora.history-command-failed.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([turn_id, actor, to]));
            e.str(command);
            e.str(reason);
            e.finish()
        }
        HistoryEntry::RetryAccepted { target, actor, .. } => {
            let mut e = Encoder::new("caelumora.history-retry-accepted.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([target, actor]));
            e.finish()
        }
        HistoryEntry::RetryRejected { target, reason, actor, .. } => {
            let mut e = Encoder::new("caelumora.history-retry-rejected.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([target, reason, actor]));
            e.finish()
        }
        HistoryEntry::ChainCutOff { message, max_agent_hops, not_asked, .. } => {
            let mut e = Encoder::new("caelumora.history-chain-cut-off.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([message, max_agent_hops, not_asked]));
            e.finish()
        }
        HistoryEntry::MessageRejected { rejected, message, reason, .. } => {
            let mut e = Encoder::new("caelumora.history-message-rejected.v1");
            e.opt_digest(previous);
            e.str(rejected);
            e.json(&serde_json::json!([message, reason]));
            e.finish()
        }
        HistoryEntry::ResponseRequested { by, .. } => {
            let mut e = Encoder::new("caelumora.history-response-requested.v1");
            e.opt_digest(previous);
            e.json(&to_json(by));
            e.finish()
        }
        // What is recorded to be delivered again (issue 223): by what it
        // says, not when.
        HistoryEntry::CommandRequested { agent, command, args, by, .. } => {
            let mut e = Encoder::new("caelumora.history-command-requested.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([agent, by]));
            e.str(command);
            e.str(args);
            e.finish()
        }
        HistoryEntry::RetryRequested { target, author, by, .. } => {
            let mut e = Encoder::new("caelumora.history-retry-requested.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([target, author, by]));
            e.finish()
        }
        HistoryEntry::ApprovalRequested { request, by, .. } => {
            let mut e = Encoder::new("caelumora.history-approval-requested.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([request, by]));
            e.finish()
        }
        HistoryEntry::ApprovalDecided { response, status, by, .. } => {
            let mut e = Encoder::new("caelumora.history-approval-decided.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([response, status, by]));
            e.finish()
        }
        HistoryEntry::ApprovalRequestWithdrawn { id, by, .. } => {
            let mut e = Encoder::new("caelumora.history-approval-request-withdrawn.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([id, by]));
            e.finish()
        }
        HistoryEntry::QuestionRequested { request, by, .. } => {
            let mut e = Encoder::new("caelumora.history-question-requested.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([request, by]));
            e.finish()
        }
        HistoryEntry::QuestionClosed { id, shown_to, by, .. } => {
            let mut e = Encoder::new("caelumora.history-question-closed.v1");
            e.opt_digest(previous);
            e.json(&serde_json::json!([id, shown_to, by]));
            e.finish()
        }
        HistoryEntry::ActivityFinished { turn_id, actor, activity, output, .. } => {
            let mut e = Encoder::new("caelumora.history-activity-finished.v1");
            e.opt_digest(previous);
            e.bytes(turn_id.0.as_bytes());
            e.json(&to_json(actor));
            e.str(activity);
            e.str(output);
            e.finish()
        }
        HistoryEntry::Compacted { turn_id, actor, from, to, .. } => {
            let mut e = Encoder::new("caelumora.history-compacted.v1");
            e.opt_digest(previous);
            e.bytes(turn_id.0.as_bytes());
            e.json(&to_json(actor));
            e.opt_digest(Some(from));
            e.opt_digest(Some(to));
            e.finish()
        }
        HistoryEntry::ParticipantJoined { participant, name, by, by_name, .. } => taking_part("caelumora.history-participant-joined.v1", previous, participant, name, by, by_name),
        HistoryEntry::ParticipantLeft { participant, name, by, by_name, until, .. } => {
            let mut e = Encoder::new("caelumora.history-participant-left.v2");
            e.opt_digest(previous);
            e.json(&to_json(participant));
            e.str(name);
            e.json(&to_json(by));
            e.str(by_name);
            e.json(&serde_json::json!(until));
            e.finish()
        }
        HistoryEntry::HistoryShared { participant, from, by, by_name, .. } => {
            let mut e = Encoder::new("caelumora.history-shared.v1");
            e.opt_digest(previous);
            e.json(&to_json(participant));
            e.json(&serde_json::json!(from));
            e.json(&to_json(by));
            e.str(by_name);
            e.finish()
        }
    }
}

/// The digest of a ledger's chain (issue 92) ending in `change`, made at
/// `origin` after message `after`, whose predecessor chain has the digest
/// `previous` - in the scheme of the ledger's kind: notes (issue 17), memory
/// (issue 62). It covers what a reader learns - the change, where in the
/// conversation it was made and, where the kind says so, the origin - not
/// when it was written.
pub fn ledger_chain<K: LedgerKind>(previous: Option<&Digest>, change: &Change<K::Id>, origin: &K::Origin, after: MessageId) -> Digest {
    let mut e = Encoder::new(K::SCHEME);
    e.opt_digest(previous);
    e.json(&to_json(change));
    if K::ORIGIN_DIGESTED {
        e.json(&to_json(origin));
    }
    e.bytes(after.0.as_bytes());
    e.finish()
}

/// The digest of the content of a file as it was read (issue 251).
pub fn content(bytes: &[u8]) -> Digest {
    let mut digest = ContentDigest::new(bytes.len() as u64);
    digest.update(bytes);
    digest.finish()
}

/// The digest of content read in parts, as [`content`] gives it of the
/// whole, for content too large to hold at once (issue 273). Its length
/// is known before its first part, as [`content`] hashes it first.
pub struct ContentDigest(Encoder);

impl ContentDigest {
    pub fn new(len: u64) -> Self {
        let mut encoder = Encoder::new("content");
        encoder.0.update(len.to_le_bytes());
        Self(encoder)
    }

    pub fn update(&mut self, part: &[u8]) {
        self.0.0.update(part);
    }

    pub fn finish(self) -> Digest {
        self.0.finish()
    }
}

/// The digest of a prompt part: the system prompt, or any other text an
/// agent's model is given besides the conversation (issue 47).
pub fn prompt(prompt: &str) -> Digest {
    let mut e = Encoder::new("caelumora.prompt.v1");
    e.str(prompt);
    e.finish()
}

/// The digest of the tool sets a model is offered, in the order it is
/// offered them (issues 137, 194): of each set its name, description and
/// whether it is unavailable, of each of its tools all of it - what the
/// model is told, and how the runtime treats its calls then.
pub fn tool_sets(sets: &[ToolSet]) -> Digest {
    let mut e = Encoder::new("caelumora.tool-sets.v2");
    e.bytes(&(sets.len() as u64).to_le_bytes());
    for set in sets {
        e.str(&set.name);
        e.str(&set.description);
        e.opt_str(set.unavailable.as_deref());
        e.bytes(&(set.tools.len() as u64).to_le_bytes());
        for tool in &set.tools {
            e.json(&to_json(tool));
        }
    }
    e.finish()
}

/// The digest of the frame of a context (issue 426): all of it - the
/// references to its prompt parts, its tools and the knowledge read, the
/// profile and the time zone -, by which it is kept once and a message names
/// it (`Message::context_frame`).
pub fn frame(frame: &FrameRef) -> Digest {
    let mut e = Encoder::new("caelumora.frame.v1");
    e.json(&to_json(frame));
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageId;
    use crate::tool::ToolCall;
    use serde_json::json;
    use chrono::{Duration, Utc};

    fn message(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
    }

    #[test]
    fn content_read_in_parts_has_the_digest_of_the_whole() {
        let whole = b"first part, second part";
        let mut parts = ContentDigest::new(whole.len() as u64);
        for part in whole.chunks(5) {
            parts.update(part);
        }
        assert_eq!(parts.finish(), content(whole));
        assert_ne!(content(b"ab"), content(b"ba"));
    }

    #[test]
    fn a_digest_reads_back_what_it_writes() {
        let digest = prompt("You are helpful.");
        let text = digest.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<Digest>().unwrap(), digest);
        assert_eq!(serde_json::from_value::<Digest>(serde_json::to_value(digest).unwrap()).unwrap(), digest);
        assert!("xyz".parse::<Digest>().is_err());
    }

    #[test]
    fn the_content_is_part_of_the_digest() {
        let original = message("hello");
        let mut changed = original.clone();
        changed.content = Some("hello!".to_string());
        assert_ne!(message_chain(None, &original), message_chain(None, &changed), "same id, other content");

        let mut described = original.clone();
        described.timestamp = Utc::now() + Duration::hours(1);
        described.prompt_tokens = Some(42);
        described.turn_id = Some(TurnId::new());
        assert_eq!(message_chain(None, &original), message_chain(None, &described), "only what the model sees counts");
    }

    #[test]
    fn the_predecessor_is_part_of_the_digest() {
        let m = message("hello");
        let a = prompt("a");
        assert_ne!(message_chain(None, &m), message_chain(Some(&a), &m));
    }

    #[test]
    fn the_order_of_json_keys_does_not_matter() {
        let with = |arguments: Value| Message {
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "f".to_string(), arguments, visibility: Default::default(), presentation: Default::default() }],
            ..message("")
        };
        let mut m1 = with(json!({"a": 1, "b": {"x": 1, "y": 2}}));
        let mut m2 = with(json!({"b": {"y": 2, "x": 1}, "a": 1}));
        m2.id = m1.id;
        m1.timestamp = m2.timestamp;
        assert_eq!(message_chain(None, &m1), message_chain(None, &m2));
    }

    #[test]
    fn the_tools_offered_are_told_apart_by_what_the_model_is_told() {
        use crate::tool::{Tool, ToolParameters, ToolSet};
        let tool = |name: &str, description: &str| Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: ToolParameters::object([("a", ToolParameters::string("x")), ("b", ToolParameters::number("y"))], ["a"]),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        };
        let set = |name: &str, tools: Vec<Tool>| ToolSet::new(name, format!("the {name} tools"), tools);
        let (a, b) = (tool("a", "does a"), tool("b", "does b"));
        let sets = |tools: Vec<Tool>| tool_sets(&[set("s", tools)]);
        assert_eq!(sets(vec![a.clone(), b.clone()]), sets(vec![a.clone(), b.clone()]), "the order of parameters in a map does not matter");
        assert_ne!(sets(vec![a.clone(), b.clone()]), sets(vec![b.clone(), a.clone()]), "the order they are offered in does");
        assert_ne!(sets(vec![a.clone()]), sets(vec![tool("a", "does something else")]), "the description does");
        assert_ne!(sets(vec![a.clone()]), sets(vec![Tool { requires_approval: true, ..a.clone() }]), "and how its calls are treated");
        assert_ne!(tool_sets(&[set("s", vec![a.clone(), b.clone()])]), tool_sets(&[set("s", vec![a.clone()]), set("t", vec![b.clone()])]), "the grouping does");
        let unavailable = ToolSet { unavailable: Some("unreachable".to_string()), ..set("s", Vec::new()) };
        assert_ne!(tool_sets(&[set("s", Vec::new())]), tool_sets(&[unavailable]), "and whether a set is unavailable");
        assert_ne!(tool_sets(&[]), prompt(""), "no tools is not an empty prompt");
    }
}
