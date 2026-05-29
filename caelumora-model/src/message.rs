use crate::agent::AgentId;
use crate::user::UserId;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use sha256::Sha256Digest;
use std::collections::HashMap;
use std::fmt::Debug;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub Uuid);


#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorId {
    None,
    User(UserId),
    Agent(AgentId)
}
impl ActorId {
    pub fn id(&self) -> Uuid {
        match &self {
            ActorId::None => {Uuid::nil()}
            ActorId::User(id) => {id.0}
            ActorId::Agent(id) => {id.0}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function_name: String,
    pub arguments: String
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub role: MessageRole,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub timestamp: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub message: Message,
    pub digest: String
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContext {
    messages: Vec<ContextEntry>
}
impl MessageContext {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }
    pub fn digest(&self) -> String {
        if self.messages.is_empty() { String::new() } else { self.messages.last().unwrap().digest.clone() }
    }
    pub fn messages(&self) -> Vec<Message> {
        self.messages.iter().map(|e| {e.message.clone()}).collect()
    }
    pub fn extend(&mut self, msgs: Vec<Message>) {
        let mut digest = self.digest();
        for m in msgs.into_iter() {
            digest = (m.id.to_string() + digest.as_str()).digest();
            self.messages.push(ContextEntry { message: m, digest: digest.clone() })
        }
    }
    pub fn push(&mut self, msg: Message) {
        let mut digest = self.digest();
        digest = (msg.id.to_string() + digest.as_str()).digest();
        self.messages.push(ContextEntry { message: msg, digest: digest })
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub message: Message,
    pub context: String,
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageHistory {
    messages: Vec<HistoryEntry>
}
impl MessageHistory {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }
    pub fn digest(&self) -> String {
        if self.messages.is_empty() { String::new() } else { self.messages.last().unwrap().digest.clone() }
    }
    pub fn messages(&self) -> Vec<Message> {
        self.messages.iter().map(|e| {e.message.clone()}).collect()
    }
    pub fn extend(&mut self, msgs: Vec<Message>) {
        let mut digest = self.digest();
        for m in msgs.into_iter() {
            digest = (m.id.to_string() + digest.as_str()).digest();
            self.messages.push(HistoryEntry { message: m, context: String::new(), digest: digest.clone() })
        }
    }
    pub fn push(&mut self, msg: Message, context:String) {
        let mut digest = self.digest();
        digest = (msg.id.to_string() + digest.as_str()).digest();
        self.messages.push(HistoryEntry { message: msg, context: context, digest: digest })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(id: MessageId) -> Message {
        Message {
            id,
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some("hello".to_string()),
            reasoning: None,
            tool_calls: Vec::new(),
            timestamp: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn message_context_empty_state() {
        let context = MessageContext::new();

        assert_eq!(context.digest(), String::new());
        assert!(context.messages().is_empty());
    }

    #[test]
    fn message_context_push_updates_recursive_digest() {
        let message_id = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap());
        let message = make_message(message_id);

        let mut context = MessageContext::new();
        context.push(message.clone());

        let expected_digest = message_id.to_string().digest();
        assert_eq!(context.digest(), expected_digest);
        assert_eq!(context.messages().len(), 1);
        assert_eq!(context.messages()[0].id, message.id);
    }

    #[test]
    fn message_context_extend_builds_recursive_digest_chain() {
        let id1 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap());
        let id2 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap());
        let id3 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap());
        let m1 = make_message(id1);
        let m2 = make_message(id2);
        let m3 = make_message(id3);

        let mut context = MessageContext::new();
        context.extend(vec![m1.clone(), m2.clone(), m3.clone()]);

        let d1 = id1.to_string().digest();
        let d2 = (id2.to_string() + d1.as_str()).digest();
        let d3 = (id3.to_string() + d2.as_str()).digest();

        assert_eq!(context.digest(), d3);

        let messages = context.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].id, m1.id);
        assert_eq!(messages[1].id, m2.id);
        assert_eq!(messages[2].id, m3.id);
    }

    #[test]
    fn message_context_extend_matches_repeated_push_digest() {
        let id1 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000011").unwrap());
        let id2 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000012").unwrap());
        let id3 = MessageId(Uuid::parse_str("00000000-0000-0000-0000-000000000013").unwrap());
        let messages = vec![make_message(id1), make_message(id2), make_message(id3)];

        let mut extend_context = MessageContext::new();
        extend_context.extend(messages.clone());

        let mut push_context = MessageContext::new();
        for message in messages {
            push_context.push(message);
        }

        assert_eq!(extend_context.digest(), push_context.digest());
        assert_eq!(extend_context.messages().len(), push_context.messages().len());
    }
}
