//! The model a test's kernel offers unless the test configures one (`UnaskedModel`, issue 520).

use crate::backend::Backends;
use crate::model::ModelProvider;
use async_trait::async_trait;
use caelumora_api::component::NoParams;
use caelumora_api::model::{ModelError, ModelResult};
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::tool::Tool;
use futures::stream::{self, BoxStream, StreamExt};
use serde_json::Value;
use std::sync::Arc;

/// The name and the backend type of the model that is not to be asked.
const UNASKED_MODEL: &str = "unasked";

/// A model that is not to be asked: a call of it fails as one of a server that cannot be reached.
/// It sees no images. The agents' tests that build components without a kernel have one of their
/// own, as a component (`caelumora_agents::testing::with_agent_kinds`): without a kernel there
/// are no models of the kernel to offer it among.
#[derive(Debug)]
struct UnaskedModel;

#[async_trait]
impl ModelProvider for UnaskedModel {
    async fn sees_images(&self) -> bool {
        false
    }

    async fn chat_completion(
        &self,
        _: &[Message],
        _: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        let refused = ModelError::Connection {
            endpoint: UNASKED_MODEL.to_string(),
            reason: "the model of a test is not to be asked".to_string(),
        };
        stream::iter([Err(refused)]).boxed()
    }
}

/// Registers the backend `unasked` with `backends`.
pub(super) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn ModelProvider, NoParams, _>(UNASKED_MODEL, |_, _, _| {
        Ok(Arc::new(UnaskedModel))
    });
}

/// `config`, with a model that is not to be asked where it names none - as a program offers its
/// own (`caelumora_ollama::plugin::with_local_model`, issue 520).
pub(super) fn with_unasked_model(mut config: AppConfig) -> AppConfig {
    if config.models.is_empty() {
        let model = BackendConfig::new(UNASKED_MODEL, Value::Null);
        config.models.insert(UNASKED_MODEL.to_string(), model);
    }
    config
}
