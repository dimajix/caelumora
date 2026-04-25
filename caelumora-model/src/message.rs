use std::collections::HashMap;
use async_trait::async_trait;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use futures::stream::BoxStream;


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
    pub id: Uuid,
    pub session_id: Uuid,
    pub sender_id: Uuid,
    pub sender_name: String,
    pub role: MessageRole,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub timestamp: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum MessageEvent {
    /// A partial piece of a message (e.g., a single sentence or a chunk of tokens)
    Partial(Message),
    /// A complete message that is now ready to be routed/processed
    Complete(Message),
    /// A signal that this agent has finished its current turn
    EndOfTurn,
}


#[async_trait]
pub trait MessageFilter: Send + Sync {
    async fn process(&self, message: Message) -> Result<Message>;
}

#[async_trait]
pub trait MessageProcessor: Send + Sync {
    async fn send_messages(&mut self, messages: Vec<Message>) -> BoxStream<'static, Result<MessageEvent>>;
}
