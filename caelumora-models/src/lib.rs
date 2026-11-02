//! The language models the platform offers (issue 520), as a plugin: a service whose backends other
//! plugins bring (`ModelProvider`), configured by name in the section `plugins.models`. An agent
//! asks such a model through a client the plugin builds for it alone (`ModelManager::client_for`),
//! which makes each call in the agent's name as the kernel holds it at that call - the one place
//! where what is called of the models the platform offers, by whom, can be known. A model an
//! agent's library brings itself is the agent's affair, and passes no such place.
//!
//! An agent is given its model as a component of the kind `model` (`component`), the default of
//! both its model's facets, `ModelClient` and `ModelProfileProvider`. Which model it names and how
//! its context is framed for that model are the component's parameters; where the model is served
//! and how is the backend's. The plugin that joins it to the kernel is `plugin`.

mod component;
mod manager;
pub mod plugin;

use async_trait::async_trait;
use caelumora_api::model::ModelResult;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// A language model the platform offers: one entry of the section `plugins.models`, served by the
/// backend its `type` names, which a plugin registers (`Backends::register`). It knows where and
/// how its model is served, and decides nothing: in whose name it is asked is the kernel's.
#[async_trait]
pub trait ModelProvider: Send + Sync + Debug {
    /// Whether the model is given images (issue 282) - configured, or asked of its server once.
    async fn sees_images(&self) -> bool;

    /// Sends `context` and the `tools` the model may call and streams its answer: partial messages
    /// as they arrive, then the complete ones, each built on `unlabelled_message` and carrying the
    /// measures of its call. What a model must not be given as a control token is defused before
    /// anything is sent (issue 149). The stream owns the request; dropping it abandons it.
    async fn chat_completion(
        &self,
        context: &[Message],
        tools: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>>;
}
