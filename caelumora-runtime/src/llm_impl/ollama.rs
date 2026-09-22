use crate::llm::{LLMClient, LLMError, LLMResult};
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::tool::ToolCall;
use chrono::Utc;
use futures::stream::{BoxStream, StreamExt};
use tracing::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
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
    parameters: OllamaFunctionParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum OllamaFunctionParameters {
    Object {
        properties: HashMap<String, OllamaFunctionParameters>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
    },
    String {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Array {
        items: Box<OllamaFunctionParameters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Boolean {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl From<caelumora_model::tool::ToolParameters> for OllamaFunctionParameters {
    fn from(value: caelumora_model::tool::ToolParameters) -> Self {
        match value {
            caelumora_model::tool::ToolParameters::Object {
                properties,
                required,
            } => OllamaFunctionParameters::Object {
                properties: properties
                    .into_iter()
                    .map(|(k, v)| (k, OllamaFunctionParameters::from(v)))
                    .collect(),
                required,
            },
            caelumora_model::tool::ToolParameters::String { description } => {
                OllamaFunctionParameters::String { description }
            }
            caelumora_model::tool::ToolParameters::Number { description } => {
                OllamaFunctionParameters::Number { description }
            }
            caelumora_model::tool::ToolParameters::Array { items, description } => {
                OllamaFunctionParameters::Array {
                    items: Box::new(OllamaFunctionParameters::from(*items)),
                    description,
                }
            }
            caelumora_model::tool::ToolParameters::Boolean { description } => {
                OllamaFunctionParameters::Boolean { description }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    function: OllamaToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCallFunction {
    index: usize,
    name: String,
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
    done: bool,
    #[serde(default)]
    total_duration: Option<u64>,
    #[serde(default)]
    load_duration: Option<u64>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    prompt_eval_duration: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    eval_duration: Option<u64>,
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
                    .enumerate()
                    .map(|(i, tc)| OllamaToolCall {
                        id: tc.id.clone(),
                        function: OllamaToolCallFunction {
                            index: i,
                            name: tc.function_name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    })
                    .collect(),
            )
        };

        if tool_calls.is_some() || value.content.is_some() || value.reasoning.is_some() {
            messages.push( Self {
                role,
                content: value.content.clone().unwrap_or_default(),
                thinking: value.reasoning.clone(),
                tool_name: None,
                tool_calls,
                tool_call_id: None
            })
        }
        for r in &value.tool_results {
            messages.push( Self {
                role: "tool".to_string(),
                content: serde_json::to_string(&r).unwrap_or_default(),
                thinking: None,
                tool_name: Some(r.tool_name.clone()),
                tool_calls: None,
                tool_call_id: r.id.clone()
            })
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::ToolCall;
    use serde_json::json;

    #[test]
    fn test_ollama_chat_message_from_with_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: Some("1".to_string()),
                function_name: "test_func_1".to_string(),
                arguments: json!("{}".to_string()),
            },
            ToolCall {
                id: Some("2".to_string()),
                function_name: "test_func_2".to_string(),
                arguments: json!("{\"a\": 1}".to_string()),
            },
        ];

        let message = Message {
            role: MessageRole::Assistant,
            tool_calls,
            ..Message::default()
        };

        let ollama_messages = OllamaChatMessage::from(&message);
        assert_eq!(ollama_messages.len(), 1);

        let ollama_msg = &ollama_messages[0];
        assert!(ollama_msg.tool_calls.is_some());

        let ollama_tool_calls = ollama_msg.tool_calls.as_ref().unwrap();
        assert_eq!(ollama_tool_calls.len(), 2);

        assert_eq!(ollama_tool_calls[0].function.name, "test_func_1");
        assert_eq!(ollama_tool_calls[0].function.index, 0);

        assert_eq!(ollama_tool_calls[1].function.name, "test_func_2");
        assert_eq!(ollama_tool_calls[1].function.index, 1);
    }

    #[test]
    fn test_ollama_chat_message_from_with_reasoning() {
        let message = Message {
            role: MessageRole::Assistant,
            content: Some("the answer".to_string()),
            reasoning: Some("let me think".to_string()),
            ..Message::default()
        };

        let ollama_messages = OllamaChatMessage::from(&message);
        assert_eq!(ollama_messages.len(), 1);
        assert_eq!(ollama_messages[0].thinking, Some("let me think".to_string()));
    }

    #[test]
    fn test_ollama_chat_request_omits_think_when_none() {
        let request = OllamaChatRequest {
            model: "test-model".to_string(),
            messages: Vec::new(),
            tools: Vec::new(),
            think: None,
            stream: true,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("think").is_none());
    }

    #[test]
    fn test_ollama_chat_request_includes_think_when_set() {
        let request = OllamaChatRequest {
            model: "test-model".to_string(),
            messages: Vec::new(),
            tools: Vec::new(),
            think: Some(true),
            stream: true,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json.get("think"), Some(&json!(true)));
    }

    #[test]
    fn test_ollama_chat_message_deserializes_thinking() {
        let raw = r#"{"role":"assistant","content":"","thinking":"pondering..."}"#;
        let msg: OllamaChatMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(msg.thinking, Some("pondering...".to_string()));

        let raw_without_thinking = r#"{"role":"assistant","content":"hi"}"#;
        let msg: OllamaChatMessage = serde_json::from_str(raw_without_thinking).unwrap();
        assert_eq!(msg.thinking, None);
    }

    #[test]
    fn test_from_env_parses_think_flag() {
        unsafe {
            env::set_var("OLLAMA_THINK", "true");
        }
        let client = OllamaClient::from_env();
        assert_eq!(client.think, Some(true));
        unsafe {
            env::remove_var("OLLAMA_THINK");
        }

        let client = OllamaClient::from_env();
        assert_eq!(client.think, None);
    }
}


/// The body of an error Ollama sends, as a whole response or as one line of
/// a stream.
#[derive(Deserialize)]
struct OllamaErrorResponse {
    error: String,
}

/// The backend's explanation from an error response body: Ollama's `error`
/// field if there is one, otherwise the start of the body.
fn backend_error_message(body: &str) -> String {
    match serde_json::from_str::<OllamaErrorResponse>(body) {
        Ok(OllamaErrorResponse { error }) => error,
        Err(_) if body.trim().is_empty() => "no details given".to_string(),
        Err(_) => body.trim().chars().take(200).collect(),
    }
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    /// Whether to request thinking/reasoning output from the model.
    /// `None` leaves the decision to Ollama's per-model default.
    think: Option<bool>,
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
            model: env::var("OLLAMA_MODEL").unwrap_or_else(|_| "gemma4:26b".to_string()),
            think: env::var("OLLAMA_THINK")
                .ok()
                .and_then(|v| v.parse::<bool>().ok()),
        }
    }

    /// Explicitly enable or disable thinking, overriding the model's default.
    pub fn with_think(mut self, think: Option<bool>) -> Self {
        self.think = think;
        self
    }
}

#[async_trait]
impl LLMClient for OllamaClient {
    async fn chat_completion(
        &self,
        context: &[Message],
        tools: &[caelumora_model::tool::Tool],
    ) -> BoxStream<'static, LLMResult<MessageEvent>> {
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
                    parameters: OllamaFunctionParameters::from(t.parameters.clone()),
                },
            })
            .collect();

        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages: context.iter().flat_map(OllamaChatMessage::from).collect(),
            tools: ollama_tools,
            think: self.think,
            stream: true,
        };

        let sender_id = ActorId::None;
        let sender_name= "ollama".to_string();

        let http = self.http.clone();
        let model = self.model.clone();
        let endpoint = LLMError::redact_endpoint(&self.base_url);

        let output = stream! {
            debug!(
                "ollama: sending chat request model={}, context={:?}, url={}",
                model,
                message_ids,
                url
            );
            let response = match http.post(url).json(&request).send().await {
                Ok(response) => response,
                Err(e) => {
                    yield Err(LLMError::Connection { endpoint, reason: LLMError::describe_chain(&e) });
                    return;
                }
            };
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                yield Err(LLMError::Rejected { endpoint, model, status: status.as_u16(), message: backend_error_message(&body) });
                return;
            }
            let mut stream = response.bytes_stream();

            let mut full_content = String::new();
            let mut full_reasoning = String::new();
            let mut all_tool_calls = Vec::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        yield Err(LLMError::Connection { endpoint, reason: LLMError::describe_chain(&e) });
                        return;
                    }
                };
                // Ollama sends multiple JSON objects, potentially split across chunks or multiple in one chunk
                // We'll use a simple approach: split by newline (Ollama usually sends one JSON per line)
                let text = String::from_utf8_lossy(&chunk);
                for line in text.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    // Ollama reports errors that occur mid-stream as `{"error": ...}`.
                    if let Ok(OllamaErrorResponse { error }) = serde_json::from_str::<OllamaErrorResponse>(line) {
                        yield Err(LLMError::Backend { endpoint, model, message: error });
                        return;
                    }
                    match serde_json::from_str::<OllamaChatResponse>(line) {
                        Ok(body) => {
                            if let Some(msg) = body.message {
                                if let Some(tc) = msg.tool_calls {
                                    for call in tc {
                                        all_tool_calls.push(ToolCall {
                                            id: call.id.or_else(|| { Some(Uuid::new_v4().to_string()) }),
                                            function_name: call.function.name,
                                            arguments: call.function.arguments,
                                        });
                                    }
                                }

                                if let Some(thinking) = msg.thinking.filter(|t| !t.is_empty()) {
                                    full_reasoning.push_str(&thinking);
                                    let partial_message = Message {
                                        id: MessageId(Uuid::new_v4()),
                                        prev_id,
                                        sender_id,
                                        sender_name: sender_name.clone(),
                                        role: MessageRole::Assistant,
                                        reasoning: Some(thinking),
                                        timestamp: Utc::now(),
                                        ..Message::default()
                                    };
                                    yield Ok(MessageEvent::Partial(partial_message));
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
                                let mut message = Message {
                                    id: MessageId(Uuid::new_v4()),
                                    prev_id,
                                    sender_id,
                                    sender_name: sender_name.clone(),
                                    role: MessageRole::Assistant,
                                    content: if full_content.is_empty() { None } else { Some(full_content.clone()) },
                                    reasoning: if full_reasoning.is_empty() { None } else { Some(full_reasoning.clone()) },
                                    tool_calls: all_tool_calls.clone(),
                                    timestamp: Utc::now(),
                                    ..Message::default()
                                };
                                // Fill in usage metrics from Ollama if available
                                message.total_duration = body.total_duration;
                                message.prompt_tokens = body.prompt_eval_count;
                                message.completion_tokens = body.eval_count;
                                message.prompt_duration = body.prompt_eval_duration;
                                message.completion_duration = body.eval_duration;

                                yield Ok(MessageEvent::Complete(message.clone()));
                                return;
                            }
                        },
                        Err(e) => {
                            // The chunk is model output: its content only at trace level.
                            warn!("ollama: failed to parse a response chunk of {} bytes: {}", line.len(), e);
                            trace!("ollama: unparseable chunk: {}", line);
                        }
                    }
                }
            }
            yield Err(LLMError::InvalidResponse { endpoint, reason: "the response ended without a final message".to_string() });
        };

        Box::pin(output)
    }
}
