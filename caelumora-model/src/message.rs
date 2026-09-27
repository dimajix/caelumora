use crate::actor::ActorId;
use crate::tool::{ToolCall, ToolCallResult, ToolVisibility};
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
    pub prev_id: MessageId,
    pub timestamp: DateTime<Utc>,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolCallResult>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// True for messages that are internal agent scaffolding - deliberation
    /// chatter, plan-step instructions, mid-plan tool calls/results - rather
    /// than a genuine final answer. Kept in `MessageContext` (this agent's
    /// own future-turn LLM input, which needs the full detail) but excluded
    /// from `MessageHistory` and end-of-turn `FullResponseGenerated`
    /// broadcasts (the user-facing/audit trail).
    #[serde(default)]
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

impl Default for Message {
    fn default() -> Self {
        Message {
            id: MessageId::default(),
            prev_id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "".to_string(),
            role: MessageRole::System,
            content: None,
            reasoning: None,
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
    /// The message as participants other than its sender see it (issue 54):
    /// tool calls and results redacted by their `ToolVisibility`. `None` if
    /// nothing is left to show - a tool message whose results are all
    /// hidden, or a message that held nothing but hidden calls.
    pub fn published(&self) -> Option<Message> {
        if self.tool_calls.iter().all(|c| c.visibility.is_public())
            && self.tool_results.iter().all(|r| r.visibility.is_public()) {
            return Some(self.clone());
        }
        let mut message = self.clone();
        message.tool_calls.retain(|c| c.visibility != ToolVisibility::Hidden);
        for call in &mut message.tool_calls {
            if call.visibility == ToolVisibility::CallOnly {
                call.arguments = serde_json::Value::Null;
            }
        }
        message.tool_results.retain(|r| r.visibility != ToolVisibility::Hidden);
        for result in &mut message.tool_results {
            if result.visibility == ToolVisibility::CallOnly {
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


pub fn format_conversation(messages:&[Message]) -> String {
    messages.iter()
        .map(|m| format!("{:?}:{}> {}", m.role, m.sender_name,  m.content.clone().unwrap_or(String::new()).as_str()))
        .reduce(|l,r| l + "\n" + r.as_str())
        .unwrap_or(String::new())
}

/// A message of a `MessageContext`, with the digest of the context up to and
/// including it (issue 06).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub message: Message,
    pub digest: Digest,
}

impl ChainEntry for ContextEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// An entry of a chain of messages.
trait MessageEntry: ChainEntry {
    fn message(&self) -> &Message;
}

impl MessageEntry for ContextEntry {
    fn message(&self) -> &Message {
        &self.message
    }
}

/// Appends `msg` to `chain` as the entry `make` builds from it and its
/// digest, and returns the digest before it. The message's `prev_id` becomes
/// the chain's last message - so it is per chain: the same message has
/// different predecessors in different chains.
fn push_message<E: MessageEntry>(chain: &mut Chain<E>, mut msg: Message, make: impl FnOnce(Message, Digest) -> E) -> Option<Digest> {
    let prev_id = last_message_id_of(chain);
    if prev_id != MessageId::default() {
        msg.prev_id = prev_id;
    }
    chain.push_with(|previous| {
        let digest = crate::digest::message_chain(previous, &msg);
        make(msg, digest)
    })
}

fn messages_of<E: MessageEntry>(chain: &Chain<E>) -> Vec<Message> {
    chain.entries().iter().map(|e| e.message().clone()).collect()
}

fn last_message_id_of<E: MessageEntry>(chain: &Chain<E>) -> MessageId {
    chain.last().map(|e| e.message().id).unwrap_or_default()
}

/// The messages an agent's model sees, as a chain: every entry's digest
/// covers its message and the digest before it, so one digest names the
/// whole context up to that point by content (see `crate::chain`).
///
/// The context is only ever appended to, so every earlier digest names a
/// prefix of it and that prefix can be recovered (`prefix`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageContext {
    messages: Chain<ContextEntry>,
}

impl MessageContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole context, or `None` while it is empty.
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
    /// context before it - the context it was produced from.
    pub fn extend(&mut self, msgs: Vec<Message>) -> Vec<Option<Digest>> {
        msgs.into_iter().map(|m| self.push(m)).collect()
    }

    /// Appends `msg` and returns the digest of the context before it.
    pub fn push(&mut self, msg: Message) -> Option<Digest> {
        push_message(&mut self.messages, msg, |message, digest| ContextEntry { message, digest })
    }

    pub fn entries(&self) -> &[ContextEntry] {
        self.messages.entries()
    }

    /// The messages of the context `digest` names: this context up to and
    /// including the entry with that digest, or `None` if it is not a prefix
    /// of this context.
    pub fn prefix(&self, digest: &Digest) -> Option<Vec<Message>> {
        Some(self.prefix_entries(digest)?.iter().map(|e| e.message.clone()).collect())
    }

    /// The entries of the context `digest` names, like [`Self::prefix`].
    pub fn prefix_entries(&self, digest: &Digest) -> Option<&[ContextEntry]> {
        self.messages.prefix(digest)
    }
}

/// A prompt part an agent's model was given besides the conversation - the
/// system prompt, notes, ... (issue 47): the digest of its text, the role it
/// was given as, and the entry of the context it followed, `None` if it came
/// before everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRef {
    pub text: Digest,
    pub role: MessageRole,
    pub after: Option<Digest>,
}

/// Knowledge from another place an agent's model was given (issue 17): what
/// was read, from which channel, as the digest of the state read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRef {
    pub channel: crate::channel::ChannelId,
    pub source: KnowledgeSource,
}

/// What a [`KnowledgeRef`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeSource {
    /// The notes an agent published in the channel; the digest of its note
    /// chain, `None` while it had none.
    Notes { agent: crate::agent::AgentId, digest: Option<Digest> },
    /// An agent's private notes in the channel (issue 51), read by that
    /// agent; the digest of its private note chain.
    PrivateNotes { agent: crate::agent::AgentId, digest: Option<Digest> },
    /// The channel's history; the digest of it when it was read.
    History { digest: Option<Digest> },
}

/// What an agent's model saw when it produced a message (issue 06): the
/// digest of its `MessageContext` before the message - `None` if it was
/// empty - the prompt parts of that turn, each where it was placed (issue
/// 47), and the knowledge from other places read before the message was
/// produced (issue 17). Resolved in the agent's `AgentChannelState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRef {
    pub context: Option<Digest>,
    #[serde(default)]
    pub prompts: Vec<PromptRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<KnowledgeRef>,
}

/// A message of a `MessageHistory`: with the digest of the history up to and
/// including it and, for a message this agent produced, what it was produced
/// from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub message: Message,
    pub context: Option<ContextRef>,
    pub digest: Digest,
}

impl ChainEntry for HistoryEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

impl MessageEntry for HistoryEntry {
    fn message(&self) -> &Message {
        &self.message
    }
}

/// The conversation as it happened - a chain like a `MessageContext`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageHistory {
    messages: Chain<HistoryEntry>,
}

impl MessageHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole history, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.messages.digest()
    }

    pub fn messages(&self) -> Vec<Message> {
        messages_of(&self.messages)
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        self.messages.entries()
    }

    pub fn last_message_id(&self) -> MessageId {
        last_message_id_of(&self.messages)
    }

    /// Appends messages that were not produced from a context of this
    /// history's owner - what others said.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m, None);
        }
    }

    /// Appends `msg` with what it was produced from, if known.
    pub fn push(&mut self, msg: Message, context: Option<ContextRef>) {
        push_message(&mut self.messages, msg, |message, digest| HistoryEntry { message, context, digest });
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::message_chain;
    use uuid::Uuid;

    fn make_message(id: MessageId) -> Message {
        Message {
            id,
            prev_id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some("hello".to_string()),
            timestamp: Utc::now(),
            ..Message::default()
        }
    }

    fn id(n: u128) -> MessageId {
        MessageId(Uuid::from_u128(n))
    }

    #[test]
    fn message_context_empty_state() {
        let context = MessageContext::new();

        assert_eq!(context.digest(), None);
        assert!(context.messages().is_empty());
    }

    #[test]
    fn message_context_chains_the_digests_of_its_messages() {
        let (m1, m2, m3) = (make_message(id(1)), make_message(id(2)), make_message(id(3)));

        let mut context = MessageContext::new();
        let before = context.extend(vec![m1.clone(), m2.clone(), m3.clone()]);

        // The chain assigns `prev_id`, which the digest does not cover.
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

        let mut extend_context = MessageContext::new();
        extend_context.extend(messages.clone());

        let mut push_context = MessageContext::new();
        for message in messages {
            push_context.push(message);
        }

        assert_eq!(extend_context.digest(), push_context.digest());
    }

    #[test]
    fn a_digest_names_the_prefix_it_was_taken_at() {
        let mut context = MessageContext::new();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);
        let at_two = context.digest().unwrap();
        context.push(make_message(id(3)));

        let prefix = context.prefix(&at_two).expect("an earlier digest names a prefix");
        assert_eq!(prefix.iter().map(|m| m.id).collect::<Vec<_>>(), vec![id(1), id(2)]);
        assert!(context.prefix(&crate::digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn digests_are_kept_as_recorded() {
        let mut context = MessageContext::new();
        context.extend(vec![make_message(id(1)), make_message(id(2))]);

        // What was recorded stands, even if today's scheme would compute
        // something else - as it will for anything recorded under an
        // earlier version of it.
        let recorded = crate::digest::prompt("recorded under another scheme");
        let mut stored = serde_json::to_value(&context).unwrap();
        stored["messages"][1]["digest"] = serde_json::Value::String(recorded.to_string());
        let loaded: MessageContext = serde_json::from_value(stored).unwrap();
        assert_eq!(loaded.digest(), Some(recorded));
        assert!(loaded.prefix(&recorded).is_some(), "and references to it resolve");
    }

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let reference = ContextRef { context: Some(message_chain(None, &make_message(id(9)))), prompts: Vec::new(), knowledge: Vec::new() };
        let mut history = MessageHistory::new();
        history.extend(vec![make_message(id(1))]);
        history.push(make_message(id(2)), Some(reference.clone()));

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(loaded.entries()[0].context, None, "said by someone else");
        assert_eq!(loaded.entries()[1].context, Some(reference));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn message_prev_id_updates() {
        let (m1, m2, m3) = (make_message(id(1)), make_message(id(2)), make_message(id(3)));

        let mut context = MessageContext::new();
        context.push(m1.clone());
        context.push(m2.clone());

        let messages = context.messages();
        assert_eq!(messages[0].prev_id, MessageId::default());
        assert_eq!(messages[1].prev_id, id(1));

        let mut history = MessageHistory::new();
        history.extend(vec![m1, m2, m3]);
        let messages = history.messages();
        assert_eq!(messages[0].prev_id, MessageId::default());
        assert_eq!(messages[1].prev_id, id(1));
        assert_eq!(messages[2].prev_id, id(2));
    }

    fn call(id: &str, visibility: ToolVisibility) -> ToolCall {
        ToolCall { id: Some(id.to_string()), function_name: "f".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility }
    }

    fn result(id: &str, visibility: ToolVisibility) -> ToolCallResult {
        ToolCallResult { id: Some(id.to_string()), tool_name: "f".to_string(), content: "secret".to_string(), status: Default::default(), visibility }
    }

    #[test]
    fn a_message_is_published_redacted_by_the_visibility_of_its_tool_calls() {
        let message = Message {
            content: None,
            tool_calls: vec![call("1", ToolVisibility::Public), call("2", ToolVisibility::CallOnly), call("3", ToolVisibility::Hidden)],
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
            tool_results: vec![result("1", ToolVisibility::Public), result("2", ToolVisibility::CallOnly), result("3", ToolVisibility::Hidden)],
            ..make_message(MessageId::new())
        };
        let published = results.published().expect("public and call-only results are left");
        let contents: Vec<_> = published.tool_results.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, ["secret", ""], "the hidden result is gone, the call-only one empty");
    }

    #[test]
    fn a_message_with_nothing_left_to_show_is_not_published() {
        let hidden_only = Message { content: None, tool_calls: vec![call("1", ToolVisibility::Hidden)], ..make_message(MessageId::new()) };
        assert!(hidden_only.published().is_none());
        let with_text = Message { tool_calls: vec![call("1", ToolVisibility::Hidden)], ..make_message(MessageId::new()) };
        let published = with_text.published().expect("the text is left");
        assert!(published.tool_calls.is_empty());
        assert_eq!(published.content.as_deref(), Some("hello"));
    }

    #[test]
    fn visibility_is_stored_only_where_it_is_not_public() {
        let public = serde_json::to_value(call("1", ToolVisibility::Public)).unwrap();
        assert!(public.get("visibility").is_none());
        let hidden = serde_json::to_value(call("1", ToolVisibility::Hidden)).unwrap();
        assert_eq!(hidden["visibility"], "hidden");
        let stored: ToolCall = serde_json::from_value(serde_json::json!({"id": "1", "function_name": "f", "arguments": {}})).unwrap();
        assert_eq!(stored.visibility, ToolVisibility::Public, "a call without it is public");
    }
}
