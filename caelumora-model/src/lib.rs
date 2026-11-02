//! The models the platform offers (issues 520, 525), as a plugin: a service whose backends other
//! plugins bring - of language models (`LanguageModelProvider`) and of image models
//! (`ImageModelProvider`) -, configured by kind and name in the section `plugins.model`. An agent
//! asks such a model through a client the plugin builds for it alone
//! (`ModelManager::language_client_for`, `image_client_for`), which makes each call in the agent's
//! name as the kernel holds it at that call - the one place where what is called of the models the
//! platform offers, by whom, can be known. A model an agent's library brings itself is the agent's
//! affair, and passes no such place.
//!
//! An agent is given its language model as a component of the kind `language_model` (module
//! `language_model`), the default of both its model's facets, `LanguageModelClient` and
//! `ModelProfileProvider`, and each image model it may use as a component of the kind
//! `image_model` (module `image_model`), which provides its `ImageModelClient`. Which model a
//! component names, and how an agent's context is framed for its language model, are the
//! components' parameters; where a model is served and how is the backend's. The plugin that joins
//! them to the kernel is `plugin`.

mod image_model;
mod language_model;
mod manager;
pub mod plugin;
#[cfg(test)]
mod testing;

use async_trait::async_trait;
use caelumora_api::image::{Image, ImageRequest};
use caelumora_api::model::ModelResult;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// A language model the platform offers: one entry of `plugins.model.language`, served by the
/// backend its `type` names, which a plugin registers (`Backends::register`). It knows where and
/// how its model is served, and decides nothing: in whose name it is asked is the kernel's.
#[async_trait]
pub trait LanguageModelProvider: Send + Sync + Debug {
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

/// An image model the platform offers (issue 525): one entry of `plugins.model.image`, served by
/// the backend its `type` names, as a language model is. It knows where and how its model is
/// served and which requests it takes, and decides nothing.
#[async_trait]
pub trait ImageModelProvider: Send + Sync + Debug {
    /// What the model is for, as a tool that offers it describes it to an agent.
    fn description(&self) -> &str;

    /// Generates the images `request` asks for. Dropping the future abandons the request to the
    /// model's server, and ends there what it began.
    ///
    /// # Errors
    /// `ModelError::Unfit` for a request the model does not take, before anything is sent; the
    /// others for what its server failed with.
    async fn generate(&self, request: &ImageRequest) -> ModelResult<Vec<Image>>;
}
