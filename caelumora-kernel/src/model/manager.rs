//! The models the kernel offers, by their names, and the clients it builds of them
//! (`ModelManager`, issue 520).

use super::ModelProvider;
use crate::backend::NamedBackends;
use crate::kernel::Acting;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::model::{ModelClient, ModelResult};
use caelumora_model::actor::ActorId;
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use std::sync::Arc;
use tracing::debug;

/// The language models the kernel offers, by the names the configuration gives them
/// (`AppConfig::models`): what builds each agent its client of one. A service of the kernel.
#[derive(Debug)]
pub(crate) struct ModelManager {
    models: NamedBackends<dyn ModelProvider>,
}

impl ModelManager {
    /// The models `models` configures.
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::SecurityContext;

    /// Issue 520: where no model is configured, none can be asked.
    #[test]
    fn where_no_model_is_configured_there_is_no_client() {
        let models = ModelManager::new(NamedBackends::default());
        let refused = models.client_for(Acting::Given(SecurityContext::default()), None);
        assert!(refused.is_err_and(|e| e.to_string().contains("no model is configured")));
    }
}
