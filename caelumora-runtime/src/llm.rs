use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent, ReasoningHistory};
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// A failure talking to an LLM backend. Every variant names the endpoint (with
/// any credentials stripped from it), so that the message is actionable on
/// its own - it is shown to users when a turn fails (issue 02).
#[derive(Debug, thiserror::Error)]
pub enum LLMError {
    /// The backend could not be reached or the connection broke.
    #[error("cannot reach LLM backend at {endpoint}: {reason}")]
    Connection { endpoint: String, reason: String },
    /// The backend answered the request with an HTTP error, e.g. an unknown
    /// model. `message` is the backend's own explanation where it gave one.
    #[error("LLM backend at {endpoint} rejected the request for model '{model}' (HTTP {status}): {message}")]
    Rejected { endpoint: String, model: String, status: u16, message: String },
    /// The backend reported an error in the middle of a response.
    #[error("LLM backend at {endpoint} failed for model '{model}': {message}")]
    Backend { endpoint: String, model: String, message: String },
    /// The response could not be understood.
    #[error("invalid response from LLM backend at {endpoint}: {reason}")]
    InvalidResponse { endpoint: String, reason: String },
}

impl LLMError {
    /// The full chain of an error's causes as one line, e.g. "error sending
    /// request: connection refused", for errors whose own `Display` stops at
    /// the outermost layer.
    pub fn describe_chain(error: &(dyn std::error::Error + 'static)) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(e) = current {
            let text = e.to_string();
            if !parts.iter().any(|p| p.contains(&text)) {
                parts.push(text);
            }
            current = e.source();
        }
        parts.join(": ")
    }

    /// `url` without user name and password, for use in messages.
    pub fn redact_endpoint(url: &str) -> String {
        match reqwest::Url::parse(url) {
            Ok(mut parsed) => {
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
                parsed.to_string().trim_end_matches('/').to_string()
            }
            Err(_) => url.to_string(),
        }
    }
}

pub type LLMResult<T> = Result<T, LLMError>;

#[async_trait]
pub trait LLMClient: Send + Sync + Debug {
    /// Sends `context` and the `tools` the model may call, and streams its
    /// answer as `MessageEvent`s: partial messages as they arrive, then the
    /// complete ones. The stream owns the request; dropping it abandons it.
    async fn chat_completion(&self, context: &[Message], tools: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>>;

    /// Which reasoning of earlier messages the model is to be given back
    /// (issue 63) - a property of the model, applied by the turn that
    /// assembles its context (`Turn::full_context`), so that `context` holds
    /// only what the model is to see.
    fn reasoning_history(&self) -> ReasoningHistory;
}