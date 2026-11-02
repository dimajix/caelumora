//! What an agent is given of a language model (issues 130, 520): the facet `ModelClient`, through
//! which it asks its model, with the errors of a call (`ModelError`), and the profile by which its
//! context is written for that model (`ModelProfileProvider`). The models the platform offers come
//! from the plugin `caelumora-model`, which builds each agent its client; a library may bring a
//! model of its own, which is then the agent's affair (issue 520).

use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::context::ModelProfile;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// A failure talking to a model's backend. Every variant names the endpoint (with any credentials
/// stripped from it), so that the message is actionable on its own - it is shown to users when a
/// turn fails (issue 02).
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The backend could not be reached or the connection broke.
    #[error("cannot reach the model server at {endpoint}: {reason}")]
    Connection { endpoint: String, reason: String },
    /// The backend answered the request with an HTTP error, e.g. an unknown
    /// model. `message` is the backend's own explanation where it gave one.
    #[error("the model server at {endpoint} rejected the request for model '{model}' (HTTP {status}): {message}")]
    Rejected { endpoint: String, model: String, status: u16, message: String },
    /// The backend reported an error in the middle of a response.
    #[error("the model server at {endpoint} failed for model '{model}': {message}")]
    Backend { endpoint: String, model: String, message: String },
    /// The response could not be understood.
    #[error("invalid response from the model server at {endpoint}: {reason}")]
    InvalidResponse { endpoint: String, reason: String },
}

/// What a call of a model answers.
pub type ModelResult<T> = Result<T, ModelError>;

/// An empty message of a model, as a `ModelClient` builds its answer from. A client does not know
/// where its answer goes; the agent classifies every message of its model before it is used (issue
/// 133). Until then it is the strictest classification there is without compartments, which no
/// channel below `strictly_confidential` admits.
pub fn unlabelled_message() -> Message {
    Message::new(Classification::general(Confidentiality::StrictlyConfidential))
}

/// The facet through which an agent asks its model, of which it has exactly one - for a model the
/// platform offers, built for that agent alone (issue 520).
#[async_trait]
pub trait ModelClient: Send + Sync + Debug {
    /// Sends `context` and the `tools` the model may call (issue 137) - of each only its name,
    /// description and parameters: whether its calls need approval, who sees them and how its
    /// results are classified are the kernel's, not the model's (issue 194) -, in the agent's name
    /// for `initiator`, who began the chain the work answers (issue 520), and streams its answer as
    /// `MessageEvent`s: partial messages as they arrive, then the complete ones, each built on
    /// `unlabelled_message`. The stream owns the request; dropping it abandons it.
    async fn chat_completion(
        &self,
        initiator: ActorId,
        context: &[Message],
        tools: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>>;
}

/// How an agent's model is to be given its context (issue 57): the profile of the model - which
/// reasoning of earlier messages it is given back, how the senders of others' messages are marked,
/// say (issues 63, 135). A property of the model, fixed for as long as its component lives, kept in
/// the context of every turn and applied where the model's input is resolved (`ContextResolver`).
/// An agent has exactly one: its model's component provides it beside its `ModelClient`, an
/// external agent's beside its `ExternalAgent` - so what writes for a model learns how without
/// asking the model itself.
pub trait ModelProfileProvider: Send + Sync + Debug {
    /// The profile of the model.
    fn profile(&self) -> ModelProfile;
}
