use crate::actor::ActorId;
use crate::confidentiality::{Classification, Visibility};
use crate::tool::{ToolCall, ToolCallResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::chain::{Chain, ChainEntry};
use crate::digest::Digest;
use std::collections::HashMap;
use std::fmt::Debug;

define_id!(MessageId);


#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub timestamp: DateTime<Utc>,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub role: MessageRole,
    /// How sensitive the message is and for whom (issue 133): set by its
    /// sender - the channel's, unless a user classifies it otherwise - and
    /// checked by the channel it enters, which admits only what may flow
    /// into it. So a message is never classified above its channel; one
    /// classified below is released to a wider audience.
    pub classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Who sees `reasoning` (issues 61, 133): the model's reasoning may
    /// weigh what the answer would not disclose, so the agent keeps it to
    /// its owner once its turn saw what only they may see.
    #[serde(default, skip_serializing_if = "Visibility::is_channel")]
    pub reasoning_visibility: Visibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolCallResult>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// True for messages that are internal agent scaffolding - deliberation
    /// chatter, the answers to plan steps, mid-plan tool calls/results - rather
    /// than a genuine final answer. Kept in `Transcript` (this agent's
    /// own future-turn LLM input, which needs the full detail) but excluded
    /// from `MessageHistory` and end-of-turn `FullResponseGenerated`
    /// broadcasts (the user-facing/audit trail).
    pub internal: bool,
    /// Participants addressed by `@`-mentions in `content`, resolved once
    /// when the message entered the channel (see `crate::mention`). Stored
    /// rather than re-parsed so that a later rename cannot change whom a past
    /// message addressed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<ActorId>,
    // Usage statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_duration: Option<u64>,        // ns
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_duration: Option<u64>,    // ns
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<u64>,         // ns
}

// No `Default`: a message has no classification that would be right
// everywhere, and a forgotten one must not pass silently (issue 133).
impl Message {
    /// An empty system message classified `classification`, for the other
    /// fields to be filled in with `..`.
    pub fn new(classification: Classification) -> Self {
        Message {
            id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "".to_string(),
            role: MessageRole::System,
            classification,
            content: None,
            reasoning: None,
            reasoning_visibility: Visibility::Channel,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            timestamp: Utc::now(),
            metadata: HashMap::new(),
            internal: false,
            mentions: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
            prompt_duration: None,
            completion_duration: None,
            total_duration: None,
        }
    }
}

impl Message {
    /// The message as its sender publishes it to a channel (issues 54,
    /// 133): every part only its owner sees (`Visibility::Owner`) is
    /// withheld - a tool call or result whose call is the owner's is left
    /// out, one whose content is has its arguments set to `null` or its
    /// result emptied, and such reasoning is dropped. `None` if nothing is
    /// left to show.
    pub fn published(&self) -> Option<Message> {
        let mut message = self.clone();
        if !message.reasoning_visibility.is_channel() {
            message.reasoning = None;
        }
        message.tool_calls.retain(|c| c.visibility.call.is_channel());
        for call in &mut message.tool_calls {
            if !call.visibility.content.is_channel() {
                call.arguments = serde_json::Value::Null;
            }
        }
        message.tool_results.retain(|r| r.visibility.call.is_channel());
        for result in &mut message.tool_results {
            if !result.visibility.content.is_channel() {
                result.content.clear();
            }
        }
        let empty = message.content.as_deref().is_none_or(str::is_empty)
            && message.reasoning.as_deref().is_none_or(str::is_empty)
            && message.tool_calls.is_empty()
            && message.tool_results.is_empty();
        (!empty).then_some(message)
    }
}

#[derive(Debug, Clone)]
pub enum MessageEvent {
    /// A partial piece of a message (e.g., a single sentence or a chunk of tokens)
    Partial(Message),
    /// A complete message that is now ready to be routed/processed
    Complete(Message),
    /// A signal that this agent has finished its current turn
    EndOfTurn(Vec<Message>),
}


/// The tool calls among `messages` that ran - their results have a start
/// (issue 145) - each with its result and the call as the model made it, if
/// it is there. What a turn given up is told to have done.
pub fn executed_calls<'a>(messages: &[&'a Message]) -> Vec<(Option<&'a ToolCall>, &'a ToolCallResult)> {
    let calls: Vec<&ToolCall> = messages.iter().flat_map(|m| &m.tool_calls).collect();
    messages.iter()
        .flat_map(|m| &m.tool_results)
        .filter(|result| result.started_at.is_some())
        .map(|result| (calls.iter().copied().find(|c| c.id.is_some() && c.id == result.id), result))
        .collect()
}

/// An entry of a chain of messages.
pub(crate) trait MessageEntry: ChainEntry {
    fn message(&self) -> &Message;
}

/// Appends `msg` to `chain` as the entry `make` builds from it and its
/// digest, and returns the digest before it.
///
/// A message's predecessor is not a property of the message: the same
/// message is in several chains, with a different predecessor in each
/// (issue 117). The chain holds it, as the digest of the entry before.
pub(crate) fn push_message<E: MessageEntry>(chain: &mut Chain<E>, msg: Message, make: impl FnOnce(Message, Digest) -> E) -> Option<Digest> {
    chain.push_with(|previous| {
        let digest = crate::digest::message_chain(previous, &msg);
        make(msg, digest)
    })
}

pub(crate) fn messages_of<E: MessageEntry>(chain: &Chain<E>) -> Vec<Message> {
    chain.entries().map(|e| e.message().clone()).collect()
}

pub(crate) fn last_message_id_of<E: MessageEntry>(chain: &Chain<E>) -> MessageId {
    chain.last().map(|e| e.message().id).unwrap_or_default()
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tool::ToolVisibility;
    use uuid::Uuid;

    /// A message from a user with id `id`.
    pub(crate) fn make_message(id: MessageId) -> Message {
        Message {
            id,
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some("hello".to_string()),
            timestamp: Utc::now(),
            ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT)
        }
    }

    /// The message id `n`.
    pub(crate) fn id(n: u128) -> MessageId {
        MessageId(Uuid::from_u128(n))
    }

    fn call(id: &str, visibility: ToolVisibility) -> ToolCall {
        ToolCall { id: Some(id.to_string()), function_name: "f".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility }
    }

    fn result(id: &str, visibility: ToolVisibility) -> ToolCallResult {
        let started_at = chrono::Utc::now();
        ToolCallResult {
            id: Some(id.to_string()), tool_name: "f".to_string(), content: "secret".to_string(), status: Default::default(), visibility, classification: crate::confidentiality::Classification::CHANNEL_DEFAULT,
            started_at: Some(started_at), finished_at: Some(started_at + chrono::Duration::milliseconds(250)),
        }
    }

    #[test]
    fn a_message_is_published_without_what_only_its_owner_sees() {
        let message = Message {
            content: None,
            tool_calls: vec![call("1", ToolVisibility::SHOWN), call("2", ToolVisibility::CALL_ONLY), call("3", ToolVisibility::HIDDEN)],
            ..make_message(MessageId::new())
        };
        let published = message.published().expect("public and call-only calls are left");
        let ids: Vec<_> = published.tool_calls.iter().map(|c| c.id.clone().unwrap()).collect();
        assert_eq!(ids, ["1", "2"], "the hidden call is gone");
        assert_eq!(published.tool_calls[0].arguments, message.tool_calls[0].arguments, "public as it is");
        assert_eq!(published.tool_calls[1].arguments, serde_json::Value::Null, "call-only without arguments");
        assert_eq!(published.id, message.id, "the same message, for the context panel to find");

        let results = Message {
            role: MessageRole::Tool,
            content: None,
            tool_results: vec![result("1", ToolVisibility::SHOWN), result("2", ToolVisibility::CALL_ONLY), result("3", ToolVisibility::HIDDEN)],
            ..make_message(MessageId::new())
        };
        let published = results.published().expect("public and call-only results are left");
        let contents: Vec<_> = published.tool_results.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, ["secret", ""], "the hidden result is gone, the call-only one empty");
        let ran = |r: &ToolCallResult| (r.started_at, r.finished_at);
        assert_eq!(ran(&published.tool_results[1]), ran(&results.tool_results[1]), "when it ran is the call's, not the content's");
    }

    #[test]
    fn only_calls_that_started_count_as_executed() {
        let calls = Message { tool_calls: vec![call("1", ToolVisibility::SHOWN), call("2", ToolVisibility::SHOWN)], ..make_message(MessageId::new()) };
        let never_ran = ToolCallResult { started_at: None, finished_at: None, ..result("2", ToolVisibility::SHOWN) };
        let results = Message { tool_results: vec![result("1", ToolVisibility::SHOWN), never_ran], ..make_message(MessageId::new()) };
        let executed = executed_calls(&[&calls, &results]);
        assert_eq!(executed.len(), 1);
        assert_eq!((executed[0].0.and_then(|c| c.id.as_deref()), executed[0].1.id.as_deref()), (Some("1"), Some("1")), "with the call it answers");
    }

    #[test]
    fn a_message_with_nothing_left_to_show_is_not_published() {
        let hidden_only = Message { content: None, tool_calls: vec![call("1", ToolVisibility::HIDDEN)], ..make_message(MessageId::new()) };
        assert!(hidden_only.published().is_none());
        let private_reasoning = Message {
            content: None,
            reasoning: Some("the note says teal".to_string()),
            reasoning_visibility: Visibility::Owner,
            ..make_message(MessageId::new())
        };
        assert!(private_reasoning.published().is_none(), "private reasoning alone is nothing to show");
        let with_text = Message { tool_calls: vec![call("1", ToolVisibility::HIDDEN)], ..make_message(MessageId::new()) };
        let published = with_text.published().expect("the text is left");
        assert!(published.tool_calls.is_empty());
        assert_eq!(published.content.as_deref(), Some("hello"));
    }

    #[test]
    fn labels_are_stored_only_where_they_are_not_public() {
        let public = serde_json::to_value(call("1", ToolVisibility::SHOWN)).unwrap();
        assert!(public.get("visibility").is_none());
        let hidden = serde_json::to_value(call("1", ToolVisibility::HIDDEN)).unwrap();
        assert_eq!(hidden["visibility"], serde_json::json!({"call": "owner", "content": "owner"}));
        let stored: ToolCall = serde_json::from_value(serde_json::json!({"id": "1", "function_name": "f", "arguments": {}})).unwrap();
        assert!(stored.visibility.is_shown(), "a call without labels is public");
        let plain = serde_json::to_value(make_message(MessageId::new())).unwrap();
        assert!(plain.get("reasoning_visibility").is_none());
    }
}
