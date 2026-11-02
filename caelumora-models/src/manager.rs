//! The models the platform offers, by their names, and the clients it builds of them
//! (`ModelManager`, issue 520).

use crate::ModelProvider;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::model::{ModelClient, ModelResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use caelumora_kernel::backend::NamedBackends;
use caelumora_kernel::kernel::Acting;
use futures::stream::BoxStream;
use std::sync::Arc;
use tracing::debug;

/// The language models the platform offers, by the names the configuration gives them (the section
/// `plugins.models`): what builds each agent its client of one. A service the plugin offers.
#[derive(Debug)]
pub(crate) struct ModelManager {
    models: NamedBackends<dyn ModelProvider>,
}

impl ModelManager {
    /// The models `models` names, of the section `plugins.models`.
    pub(crate) fn new(models: NamedBackends<dyn ModelProvider>) -> Self {
        Self { models }
    }

    /// The client of the model `name` - the one there is where `None` -, for whoever `acting` acts
    /// for alone, each call made in the context `acting` builds at that call.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` naming the parameter `name` where it names no model, or none
    /// among several, or where no model is configured.
    pub(crate) fn client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<ModelClientFor> {
        let chosen = self
            .models
            .chosen(name)
            .map_err(|why| ComponentError::InvalidConfig(format!("'name' {why}")))?;
        let Some((name, model)) = chosen else {
            return Err(ComponentError::InvalidConfig("no model is configured".to_string()));
        };
        Ok(ModelClientFor { acting, name: name.to_string(), model: Arc::clone(model) })
    }
}

/// A model as one agent asks it (`ModelManager::client_for`): each call made in the context its
/// `Acting` builds at that call, which says who asks which model - what a check of the call, or a
/// count of what it used, is to go by.
#[derive(Debug)]
pub(crate) struct ModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn ModelProvider>,
}

impl ModelClientFor {
    /// Whether its model is given images (`ModelProvider::sees_images`).
    pub(crate) async fn sees_images(&self) -> bool {
        self.model.sees_images().await
    }
}

#[async_trait]
impl ModelClient for ModelClientFor {
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
