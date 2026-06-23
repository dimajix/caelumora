use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole};
use chrono::Utc;
use futures::stream::{BoxStream, StreamExt};
use log::{debug, warn};
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
    done: bool,
}

impl OllamaChatMessage {
    fn from(value: &Message) -> Vec<Self> {
        let mut messages = Vec::new();
        let role = match value.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        }
        .to_string();

        let tool_calls = if value.tool_calls.is_empty() {
            None
        } else {
            Some(
                value
                    .tool_calls
                    .iter()
                    .map(|tc| OllamaToolCall {
                        function: OllamaToolCallFunction {
                            name: tc.function_name.clone(),
                            arguments: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
                        },
                    })
                    .collect(),
            )
        };

        if tool_calls.is_some() || value.content.is_some() {
            messages.push( Self {
                role,
                content: value.content.clone().unwrap_or_default(),
                tool_name: None,
                tool_calls
            })
        }
        for r in &value.tool_results {
            messages.push( Self {
                role: "tool".to_string(),
                content: serde_json::to_string(&r).unwrap_or_default(),
                tool_name: Some(r.tool_name.clone()),
                tool_calls: None,
            })
        }

        messages
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
        tools: &[caelumora_model::tool::Tool],
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let message_ids: Vec<_> = context.iter().map(|m| m.id).collect();
        let prev_id = context.iter().map(|m| m.id).last().unwrap_or(MessageId::default());

        let ollama_tools: Vec<_> = tools
            .iter()
            .map(|t| OllamaTool {
                tool_type: "function".to_string(),
                function: OllamaFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages: context.iter().flat_map(OllamaChatMessage::from).collect(),
            tools: ollama_tools,
            stream: true,
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
            let mut stream = response.error_for_status()?.bytes_stream();

            let mut full_content = String::new();
            let mut all_tool_calls = Vec::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result?;
                // Ollama sends multiple JSON objects, potentially split across chunks or multiple in one chunk
                // We'll use a simple approach: split by newline (Ollama usually sends one JSON per line)
                let text = String::from_utf8_lossy(&chunk);
                for line in text.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<OllamaChatResponse>(line) {
                        Ok(body) => {
                            if let Some(msg) = body.message {
                                if let Some(tc) = msg.tool_calls {
                                    for call in tc {
                                        all_tool_calls.push(caelumora_model::message::ToolCall {
                                            id: Uuid::new_v4().to_string(),
                                            function_name: call.function.name,
                                            arguments: call.function.arguments.to_string(),
                                        });
                                    }
                                }

                                if !msg.content.is_empty() {
                                    full_content.push_str(&msg.content);
                                    let partial_message = Message {
                                        id: MessageId(Uuid::new_v4()),
                                        prev_id,
                                        sender_id,
                                        sender_name: sender_name.clone(),
                                        role: MessageRole::Assistant,
                                        content: Some(msg.content),
                                        timestamp: Utc::now(),
                                        ..Message::default()
                                    };
                                    yield Ok(MessageEvent::Partial(partial_message));
                                }
                            }
                            if body.done {
                                let message = Message {
                                    id: MessageId(Uuid::new_v4()),
                                    prev_id,
                                    sender_id,
                                    sender_name: sender_name.clone(),
                                    role: MessageRole::Assistant,
                                    content: if full_content.is_empty() { None } else { Some(full_content.clone()) },
                                    tool_calls: all_tool_calls.clone(),
                                    timestamp: Utc::now(),
                                    ..Message::default()
                                };

                                yield Ok(MessageEvent::Complete(message.clone()));
                                return;
                            }
                        },
                        Err(e) => {
                            warn!("ollama: failed to parse JSON chunk: {} (chunk: {})", e, line);
                        }
                    }
                }
            }
        };

        Box::pin(output)
    }
}
