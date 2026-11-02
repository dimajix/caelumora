//! The models the platform offers, by their kind and their names, and the clients it builds of
//! them (`ModelManager`, issues 520, 525).

use crate::{ImageModelProvider, LanguageModelProvider};
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::image::{Image, ImageModelClient, ImageRequest};
use caelumora_api::model::{LanguageModelClient, ModelResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use caelumora_kernel::backend::NamedBackends;
use caelumora_kernel::kernel::Acting;
use futures::stream::BoxStream;
use std::sync::Arc;
use tracing::debug;

/// The models the platform offers, by the names the configuration gives them
/// (`plugins.model.language`, `plugins.model.image`): what builds each agent its clients of them.
/// A service the plugin offers.
#[derive(Debug)]
pub(crate) struct ModelManager {
    language: NamedBackends<dyn LanguageModelProvider>,
    image: NamedBackends<dyn ImageModelProvider>,
}

impl ModelManager {
    /// The language models `language` and the image models `image` name.
    pub(crate) fn new(
        language: NamedBackends<dyn LanguageModelProvider>,
        image: NamedBackends<dyn ImageModelProvider>,
    ) -> Self {
        Self { language, image }
    }

    /// The client of the language model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone, each call made in the context `acting` builds at that call.
    ///
    /// # Errors
    /// As `chosen`.
    pub(crate) fn language_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<LanguageModelClientFor> {
        let (name, model) = chosen(&self.language, name, "language model")?;
        Ok(LanguageModelClientFor { acting, name: name.to_string(), model: Arc::clone(model) })
    }

    /// The client of the image model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone, each call made in the context `acting` builds at that call.
    ///
    /// # Errors
    /// As `chosen`.
    pub(crate) fn image_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<ImageModelClientFor> {
        let (name, model) = chosen(&self.image, name, "image model")?;
        Ok(ImageModelClientFor { acting, name: name.to_string(), model: Arc::clone(model) })
    }
}

/// The model of `models` that the parameter `name` of a component names - the one there is where
/// `None` -, with its name; `what` names the kind of model in messages.
///
/// # Errors
/// `ComponentError::InvalidConfig` naming the parameter `name` where it names no model, or none
/// among several, or where no model of the kind is configured.
fn chosen<'a, T: ?Sized>(
    models: &'a NamedBackends<T>,
    name: Option<&str>,
    what: &str,
) -> ComponentResult<(&'a str, &'a Arc<T>)> {
    let chosen = models
        .chosen(name)
        .map_err(|why| ComponentError::InvalidConfig(format!("'name' {why}")))?;
    chosen.ok_or_else(|| ComponentError::InvalidConfig(format!("no {what} is configured")))
}

/// A language model as one agent asks it (`ModelManager::language_client_for`): each call made in
/// the context its `Acting` builds at that call, which says who asks which model - what a check of
/// the call, or a count of what it used, is to go by.
#[derive(Debug)]
pub(crate) struct LanguageModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn LanguageModelProvider>,
}

impl LanguageModelClientFor {
    /// Whether its model is given images (`LanguageModelProvider::sees_images`).
    pub(crate) async fn sees_images(&self) -> bool {
        self.model.sees_images().await
    }
}

#[async_trait]
impl LanguageModelClient for LanguageModelClientFor {
    async fn chat_completion(
        &self,
        initiator: ActorId,
        context: &[Message],
        tools: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        let asking = self.acting.context_for(initiator);
        debug!(
            "model {} asked by {:?} for {:?}, initiated by {:?}",
            self.name, asking.caller_id, asking.principal_id, asking.initiator_id
        );
        self.model.chat_completion(context, tools).await
    }
}

/// An image model as one agent asks it (`ModelManager::image_client_for`): each call made in the
/// context its `Acting` builds at that call, as a language model's is.
#[derive(Debug)]
pub(crate) struct ImageModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn ImageModelProvider>,
}

#[async_trait]
impl ImageModelClient for ImageModelClientFor {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        self.model.description()
    }

    async fn generate(
        &self,
        initiator: ActorId,
        request: &ImageRequest,
    ) -> ModelResult<Vec<Image>> {
        let asking = self.acting.context_for(initiator);
        debug!(
            "image model {} asked by {:?} for {:?}, initiated by {:?}",
            self.name, asking.caller_id, asking.principal_id, asking.initiator_id
        );
        self.model.generate(request).await
    }
}
