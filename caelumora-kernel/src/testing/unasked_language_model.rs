//! The model a test's agents talk to unless the test gives them another (`UnaskedLanguageModel`,
//! issue 520): the kernel offers no model of its own - the models are a plugin's -, but an agent
//! has one.

use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::model::{LanguageModelClient, ModelError, ModelProfileProvider, ModelResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::context::ModelProfile;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use futures::stream::{self, BoxStream, StreamExt};
use std::sync::Arc;

/// The kind of the model that is not to be asked.
const UNASKED_LANGUAGE_MODEL_KIND: &str = "unasked_language_model";

/// A model that is not to be asked: a call of it fails as one of a server that cannot be reached.
/// Its context is framed by the defaults, and it sees no images.
#[derive(Debug)]
struct UnaskedLanguageModel;

#[async_trait]
impl LanguageModelClient for UnaskedLanguageModel {
    async fn chat_completion(
        &self,
        _: ActorId,
        _: &[Message],
        _: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        let refused = ModelError::Connection {
            endpoint: UNASKED_LANGUAGE_MODEL_KIND.to_string(),
            reason: "the model of a test is not to be asked".to_string(),
        };
        stream::iter([Err(refused)]).boxed()
    }
}

impl ModelProfileProvider for UnaskedLanguageModel {
    fn profile(&self) -> ModelProfile {
        ModelProfile::default()
    }
}

/// Registers with `registry` a model that is not to be asked, declared the default of the model's
/// facets: for a test's kernel (`TestKernel`), whose plugins may declare another after it - the
/// models' -, and for a test that builds an agent's components without a kernel.
pub fn with_unasked_language_model(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(UNASKED_LANGUAGE_MODEL_KIND)
            .provides::<dyn LanguageModelClient>()
            .provides::<dyn ModelProfileProvider>(),
        |_, _| {
            let model = Arc::new(UnaskedLanguageModel);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn LanguageModelClient>(Arc::<UnaskedLanguageModel>::clone(&model));
            facets.insert::<dyn ModelProfileProvider>(model);
            Ok(facets)
        },
    );
    registry.single::<dyn LanguageModelClient>(Some(UNASKED_LANGUAGE_MODEL_KIND));
    registry.single::<dyn ModelProfileProvider>(Some(UNASKED_LANGUAGE_MODEL_KIND));
}
