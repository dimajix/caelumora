//! Content digests (issue 06): SHA-256 hashes that identify what an agent
//! saw - a chain of messages, a system prompt - by their content.
//!
//! A `Digest` is written as 64 lowercase hex digits, in JSON as well, so that
//! stored state stays readable. What goes into a digest is defined here, in
//! one place, with a version tag in front of it, so that the scheme can
//! change without old and new digests ever being mistaken for each other.

use crate::message::Message;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::fmt;
use std::str::FromStr;

/// A SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

impl Digest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
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
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Feeds values into a hash unambiguously: every value is preceded by its
/// length, so that no two different sequences produce the same input.
struct Encoder(Sha256);

impl Encoder {
    fn new(scheme: &str) -> Self {
        let mut encoder = Encoder(Sha256::new());
        encoder.bytes(scheme.as_bytes());
        encoder
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }

    fn str(&mut self, s: &str) {
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

    fn finish(self) -> Digest {
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

fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The digest of a chain of messages ending in `message`, whose predecessor
/// chain has the digest `previous` (`None` for the first message).
///
/// It covers what the model sees of a message - id, role, sender, content,
/// reasoning, tool calls and results, whether it is internal - and not what
/// only describes it afterwards: timestamps, usage statistics, resolved
/// mentions, metadata, and `prev_id`, which the chain already expresses.
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

/// The digest of a system prompt.
pub fn system_prompt(prompt: &str) -> Digest {
    let mut e = Encoder::new("caelumora.system-prompt.v1");
    e.str(prompt);
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageId;
    use crate::tool::ToolCall;
    use serde_json::json;

    fn message(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() }
    }

    #[test]
    fn a_digest_reads_back_what_it_writes() {
        let digest = system_prompt("You are helpful.");
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
        described.timestamp = chrono::Utc::now() + chrono::Duration::hours(1);
        described.prompt_tokens = Some(42);
        assert_eq!(message_chain(None, &original), message_chain(None, &described), "only what the model sees counts");
    }

    #[test]
    fn the_predecessor_is_part_of_the_digest() {
        let m = message("hello");
        let a = system_prompt("a");
        assert_ne!(message_chain(None, &m), message_chain(Some(&a), &m));
    }

    #[test]
    fn the_order_of_json_keys_does_not_matter() {
        let with = |arguments: Value| Message {
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "f".to_string(), arguments }],
            ..message("")
        };
        let mut m1 = with(json!({"a": 1, "b": {"x": 1, "y": 2}}));
        let mut m2 = with(json!({"b": {"y": 2, "x": 1}, "a": 1}));
        m2.id = m1.id;
        m1.timestamp = m2.timestamp;
        assert_eq!(message_chain(None, &m1), message_chain(None, &m2));
    }
}
