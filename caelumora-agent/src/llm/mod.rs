//! Language models (issue 130): the facet `LLMClient`, through which an
//! agent asks its model, and what any kind that provides it may use - the
//! defusing of control tokens (`control_tokens`). The kinds themselves are
//! in crates of their own, and the program names the default (issue 329).

pub mod control_tokens;

use caelumora_runtime::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::context::ModelProfile;
use caelumora_model::message::{Message, MessageEvent};
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

pub type LLMResult<T> = Result<T, LLMError>;

/// An empty message of a model, as an `LLMClient` builds its answer from.
/// A client does not know where its answer goes; the agent classifies every
/// message of its model before it is used (`actor::agent::label`, issue 133).
/// Until then it is the strictest classification there is without
/// compartments, which no channel below `strictly_confidential` admits.
pub fn unlabelled_message() -> Message {
    Message::new(Classification::general(Confidentiality::StrictlyConfidential))
}

#[async_trait]
pub trait LLMClient: Send + Sync + Debug {
    /// Sends `context` and the `tools` the model may call (issue 137) - of
    /// each only its name, description and parameters: whether its calls
    /// need approval, who sees them and how its results are classified are
    /// the runtime's, not the model's (issue 194) - and streams its answer
    /// as `MessageEvent`s:
    /// partial messages as they arrive, then the complete ones, each built
    /// on `unlabelled_message`. The stream owns the request; dropping it
    /// abandons it.
    async fn chat_completion(&self, context: &[Message], tools: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>>;

    /// How the model is to be given its context - which reasoning of
    /// earlier messages, say (issue 63). A property of the model, fixed in
    /// the context of every turn and applied where the model's input is
    /// resolved (`ContextResolver`), so that `context` holds only what the
    /// model is to see.
    fn profile(&self) -> ModelProfile;
}

/// Registers the `LLMClient` with `registry` as a facet of which an agent
/// has exactly one - without a default kind: the core knows no model
/// provider, the program names it (`ComponentRegistry::default_kind`,
/// issue 329).
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn LLMClient>(None);
}
