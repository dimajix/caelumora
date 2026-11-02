//! The language models the kernel offers (issue 520): a service of the kernel whose backends the
//! plugins bring (`ModelProvider`), configured by name (`AppConfig::models`). Every call of a model
//! an agent's work makes passes the kernel: an agent asks its model through a client the kernel
//! builds for it alone (`ModelManager::client_for`), which makes each call in the agent's name as
//! the kernel holds it at that call - the one place where what is called, by whom, can be known.
//!
//! An agent is given its model as a component of the kernel's kind `model` (`component`), the
//! default of both its model's facets, `ModelClient` and `ModelProfileProvider`; no library may
//! bring a `ModelClient` of its own (`KernelBuilder::build`). Which model it names and how its
//! context is framed for that model are the component's parameters; where the model is served and
//! how is the backend's.

mod component;
mod manager;

pub use component::MODEL_KIND;
pub(crate) use manager::ModelManager;

use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::model::{ModelClient, ModelProfileProvider, ModelResult};
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// A language model the kernel offers: one entry of the configuration's `models`, served by the
/// backend its `type` names - a backend of the kernel's models, which a plugin registers
/// (`Backends::register`). It knows where and how its model is served, and decides nothing: in
/// whose name it is asked is the kernel's.
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

/// Registers the kind `model` with `registry`, and the model's facets, `ModelClient` and
/// `ModelProfileProvider`, as facets of which an agent has exactly one, with it as their default.
pub(crate) fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn ModelClient>(Some(MODEL_KIND));
    registry.single::<dyn ModelProfileProvider>(Some(MODEL_KIND));
    component::register(registry);
}
