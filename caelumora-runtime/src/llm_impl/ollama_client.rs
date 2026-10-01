//! The `ollama_client` component: the language model an agent talks to,
//! served by Ollama (issue 130). It provides the `LLMClient` facet, of
//! which an agent has exactly one: its personality's, the one
//! `AppConfig::component_defaults` names, or this kind with its defaults -
//! a local Ollama and the model `DEFAULT_MODEL`.
//!
//! Parameters: `base_url` (the Ollama server), `model`, `think` - whether
//! the model is asked to reason; left out, Ollama's default for the model
//! applies - `reasoning_history`, which reasoning of earlier messages the
//! model is given back (`never`, `current_turn` - the default - or
//! `always`, issue 63), and `system_messages`, where the model keeps system
//! messages (`only_at_start` - the default - or `anywhere`, issue 49). An
//! unknown parameter is refused, so that a misspelt one does not silently
//! fall back to its default.

use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::llm::{unlabelled_message, LLMClient, LLMError, LLMResult};
use async_stream::stream;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::context::{ModelProfile, ReasoningHistory, SystemMessages};
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::tool::{ToolCall, ToolVisibility};
use chrono::Utc;
use futures::stream::{BoxStream, StreamExt};
use tracing::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

pub const OLLAMA_CLIENT_KIND: &str = "ollama_client";

/// The Ollama server asked unless `base_url` names another.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
/// The model asked unless `model` names another.
pub const DEFAULT_MODEL: &str = "gemma4:26b";

/// The parameters of an `ollama_client` component.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OllamaClientConfig {
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default)]
    think: Option<bool>,
    #[serde(default)]
    reasoning_history: ReasoningHistory,
    #[serde(default)]
    system_messages: SystemMessages,
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    /// Whether to request thinking/reasoning output from the model.
    /// `None` leaves the decision to Ollama's per-model default.
    think: Option<bool>,
    /// How the model is given its context (issue 63).
    profile: ModelProfile,
}

impl OllamaClient {
    fn new(config: OllamaClientConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: config.base_url,
            model: config.model,
            think: config.think,
            profile: ModelProfile { reasoning_history: config.reasoning_history, system_messages: config.system_messages },
        }
    }
}

#[async_trait]
impl LLMClient for OllamaClient {
    async fn chat_completion(
        &self,
        context: &[Message],
        tools: &[caelumora_model::tool::ToolDefinition],
    ) -> BoxStream<'static, LLMResult<MessageEvent>> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let message_count = context.len();

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
                "ollama: sending chat request model={}, messages={}, endpoint={}",
                model,
                message_count,
                endpoint
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
                                            visibility: ToolVisibility::SHOWN,
                                        });
                                    }
                                }

                                if let Some(thinking) = msg.thinking.filter(|t| !t.is_empty()) {
                                    full_reasoning.push_str(&thinking);
                                    let partial_message = Message {
                                        id: MessageId(Uuid::new_v4()),
                                        sender_id,
                                        sender_name: sender_name.clone(),
                                        role: MessageRole::Assistant,
                                        reasoning: Some(thinking),
                                        timestamp: Utc::now(),
                                        ..unlabelled_message()
                                    };
                                    yield Ok(MessageEvent::Partial(partial_message));
                                }

                                if !msg.content.is_empty() {
                                    full_content.push_str(&msg.content);
                                    let partial_message = Message {
                                        id: MessageId(Uuid::new_v4()),
                                        sender_id,
                                        sender_name: sender_name.clone(),
                                        role: MessageRole::Assistant,
                                        content: Some(msg.content),
                                        timestamp: Utc::now(),
                                        ..unlabelled_message()
                                    };
                                    yield Ok(MessageEvent::Partial(partial_message));
                                }
                            }
                            if body.done {
                                let mut message = Message {
                                    id: MessageId(Uuid::new_v4()),
                                    sender_id,
                                    sender_name: sender_name.clone(),
                                    role: MessageRole::Assistant,
                                    content: if full_content.is_empty() { None } else { Some(full_content.clone()) },
                                    reasoning: if full_reasoning.is_empty() { None } else { Some(full_reasoning.clone()) },
                                    tool_calls: all_tool_calls.clone(),
                                    timestamp: Utc::now(),
                                    ..unlabelled_message()
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

    fn profile(&self) -> ModelProfile {
        self.profile
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(OLLAMA_CLIENT_KIND).provides::<dyn LLMClient>(),
        |descriptor, _context| {
            // Built as the default, or configured without parameters: all
            // parameters take their defaults.
            let params = if descriptor.params.is_null() { serde_json::json!({}) } else { descriptor.params.clone() };
            let config: OllamaClientConfig = serde_json::from_value(params)
                .map_err(|e| ComponentError::InvalidConfig(format!("invalid parameters: {e}")))?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn LLMClient>(std::sync::Arc::new(OllamaClient::new(config)));
            Ok(facets)
        },
    );
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
                visibility: Default::default(),
            },
            ToolCall {
                id: Some("2".to_string()),
                function_name: "test_func_2".to_string(),
                arguments: json!("{\"a\": 1}".to_string()),
                visibility: Default::default(),
            },
        ];

        let message = Message {
            role: MessageRole::Assistant,
            tool_calls,
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
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
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
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
    fn parameters_default_to_a_local_ollama() {
        let config: OllamaClientConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.think, None);
        assert_eq!(config.reasoning_history, ReasoningHistory::CurrentTurn);
        assert_eq!(config.system_messages, SystemMessages::OnlyAtStart);

        let config: OllamaClientConfig = serde_json::from_value(json!({ "model": "qwen3:8b", "think": true, "reasoning_history": "never", "system_messages": "anywhere" })).unwrap();
        assert_eq!((config.model.as_str(), config.think), ("qwen3:8b", Some(true)));
        assert_eq!(config.reasoning_history, ReasoningHistory::Never);
        assert_eq!(config.system_messages, SystemMessages::Anywhere);
    }

    #[test]
    fn a_misspelt_parameter_is_refused() {
        assert!(serde_json::from_value::<OllamaClientConfig>(json!({ "modle": "qwen3:8b" })).is_err());
    }
}
