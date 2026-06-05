use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole};
use chrono::Utc;
use futures::stream::BoxStream;
use log::{debug};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use uuid::Uuid;

use crate::llm::LLMClient;
use crate::message::MessageEvent;

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
}

impl From<&Message> for OllamaChatMessage {
    fn from(value: &Message) -> Self {
        let role = match value.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        }
        .to_string();

        Self {
            role,
            content: value.content.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl OllamaClient {
    pub fn from_env() -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            model: env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".to_string()),
        }
    }
}

#[async_trait]
impl LLMClient for OllamaClient {
    async fn chat_completion(
        &self,
        context: &[Message],
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let message_ids: Vec<_> = context.iter().map(|m| m.id).collect();
        let prev_id = context.iter().map(|m| m.id).last().unwrap_or(MessageId::default());
        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages: context.iter().map(OllamaChatMessage::from).collect(),
            stream: false,
        };

        let sender_id = ActorId::None;
        let sender_name= "ollama".to_string();

        let http = self.http.clone();
        let model = self.model.clone();

        let output = stream! {
            debug!(
                "ollama: sending chat request model={}, context={:?}, url={}",
                model,
                message_ids,
                url
            );
            let response = http.post(url).json(&request).send().await?;
            let body: OllamaChatResponse = response.error_for_status()?.json().await?;
            let content = body
                .message
                .and_then(|msg| if msg.content.is_empty() { None } else { Some(msg.content) });

            debug!(
                "ollama: received chat response model={}, context={:?}, content={:?}",
                model,
                request.messages,
                content
            );

            let message = Message {
                id: MessageId(Uuid::new_v4()),
                prev_id,
                sender_id,
                sender_name,
                role: MessageRole::Assistant,
                content,
                reasoning: None,
                tool_calls: vec![],
                timestamp: Utc::now(),
                metadata: HashMap::new(),
            };

            yield Ok(MessageEvent::Complete(message.clone()));
            yield Ok(MessageEvent::EndOfTurn(vec![message]));
        };

        Box::pin(output)
    }
}
