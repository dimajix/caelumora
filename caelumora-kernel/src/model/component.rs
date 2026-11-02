//! The kind `model` (issue 520): an agent's language model, one of those the kernel offers, as the
//! agent is given it - its client, which the kernel builds for this agent alone, and the profile by
//! which its context is framed for that model.
//!
//! Parameters: `name`, the model of the configuration's `models` - left out, the one there is -,
//! and how the context is framed for it: `reasoning_history`, which reasoning of earlier messages
//! the model is given back (`never`, `current_turn` - the default - or `always`, issue 63),
//! `system_messages`, where the model keeps system messages (`only_at_start` - the default - or
//! `anywhere`, issue 49), `sender_format` and `sender_style`, how the model is told who wrote a
//! message of another participant (`none`, `prefix` - the default - or `xml`; by `handle` - the
//! default - or `name`, issue 135), `message_times`, whether the marker says when the message was
//! written, in the session's time zone (`true` - the default - or `false`, issue 424), and
//! `prompt_texts`, the variant of the texts its model is given (issue 57; the default unless
//! configured). Whether the model sees images is the model's, asked as the component is built
//! (issue 282). An unknown parameter is refused, so that a misspelt one does not silently fall back
//! to its default.

use super::ModelManager;
use crate::component::ComponentRegistry;
use crate::kernel::Acting;
use caelumora_api::component::{config_of_params, ComponentFacets, ComponentKind, Params};
use caelumora_api::model::{ModelClient, ModelProfileProvider};
use caelumora_model::context::{
    message_times_by_default, ModelProfile, ReasoningHistory, SenderFormat, SenderStyle,
    SystemMessages,
};
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptVariant};
use serde::Deserialize;
use std::sync::Arc;

/// The kind of an agent's model: the default of both its facets.
pub const MODEL_KIND: &str = "model";

/// The parameters of a `model` component.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelParams {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reasoning_history: ReasoningHistory,
    #[serde(default)]
    system_messages: SystemMessages,
    #[serde(default = "SenderFormat::configured_default")]
    sender_format: SenderFormat,
    #[serde(default)]
    sender_style: SenderStyle,
    #[serde(default = "message_times_by_default")]
    message_times: bool,
    #[serde(default)]
    prompt_texts: Option<PromptVariant>,
}

impl Params for ModelParams {}

/// How the context of the agent's model is framed, as its component is configured.
#[derive(Debug)]
struct Framing(ModelProfile);

impl ModelProfileProvider for Framing {
    fn profile(&self) -> ModelProfile {
        self.0
    }
}

/// Registers the kind `model` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(MODEL_KIND)
            .params::<ModelParams>()
            .provides::<dyn ModelClient>()
            .provides::<dyn ModelProfileProvider>()
            .uses::<ModelManager>()
            .uses::<Acting>()
            .uses::<PromptCatalogue>(),
        |descriptor, context| async move {
            let params = config_of_params::<ModelParams>(&descriptor.params)?;
            context.service::<PromptCatalogue>()?.require_variant(params.prompt_texts)?;
            // For the agent its components are built for, in its name at each call.
            let acting = Acting::clone(context.service::<Acting>()?);
            let models = context.service::<ModelManager>()?;
            let client = models.client_for(acting, params.name.as_deref())?;
            let profile = ModelProfile {
                reasoning_history: params.reasoning_history,
                system_messages: params.system_messages,
                sender_format: params.sender_format,
                sender_style: params.sender_style,
                message_times: params.message_times,
                prompt_texts: params.prompt_texts,
                images: client.sees_images().await,
            };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ModelClient>(Arc::new(client));
            facets.insert::<dyn ModelProfileProvider>(Arc::new(Framing(profile)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backends;
    use crate::component::BuildContext;
    use crate::kernel::{Kernel, KernelBuilder};
    use crate::model::ModelProvider;
    use crate::plugin::Plugin;
    use crate::testing::TestKernel;
    use anyhow::Result;
    use async_trait::async_trait;
    use caelumora_api::component::{ComponentContainer, ComponentError, ComponentResult};
    use caelumora_api::model::{unlabelled_message, ModelResult};
    use caelumora_model::actor::ActorId;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::config::{AppConfig, BackendConfig};
    use caelumora_model::message::{Message, MessageEvent};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::Tool;
    use caelumora_model::type_map::TypeKey;
    use futures::stream::{self, BoxStream, StreamExt};
    use serde_json::{json, Value};
    use std::path::PathBuf;

    /// What a model of the backend `answering` is configured with.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        answer: String,
        #[serde(default)]
        seeing: bool,
    }

    impl Params for Answer {}

    /// A model that answers what it is configured to, whatever it is asked.
    #[derive(Debug)]
    struct Answering(Answer);

    #[async_trait]
    impl ModelProvider for Answering {
        async fn sees_images(&self) -> bool {
            self.0.seeing
        }

        async fn chat_completion(
            &self,
            _: &[Message],
            _: &[Tool],
        ) -> BoxStream<'static, ModelResult<MessageEvent>> {
            let answer = Message { content: Some(self.0.answer.clone()), ..unlabelled_message() };
            stream::iter([Ok(MessageEvent::Complete(answer))]).boxed()
        }
    }

    /// Brings the backend `answering`.
    struct Answerers;

    impl Plugin for Answerers {
        fn name(&self) -> &'static str {
            "answerers"
        }

        fn backends(&self, backends: &mut Backends) {
            backends.register::<dyn ModelProvider, Answer, _>("answering", |_, answer, _| {
                Ok(Arc::new(Answering(answer)))
            });
        }

        fn register(&self, _: &mut KernelBuilder) -> Result<()> {
            Ok(())
        }
    }

    /// The kernel of a test that offers the models `models`, each answering with its `answer`, and
    /// the variants of the texts `prompts`.
    fn offering(models: &[(&str, Value)], prompts: &[(&str, PathBuf)]) -> Kernel {
        let models = models
            .iter()
            .map(|(name, params)| {
                (name.to_string(), BackendConfig::new("answering", params.clone()))
            })
            .collect();
        let prompts =
            prompts.iter().map(|(variant, dir)| (variant.to_string(), dir.clone())).collect();
        let config = AppConfig { models, prompts, ..AppConfig::default() };
        TestKernel { config, plugins: vec![&Answerers], ..Default::default() }.try_build().unwrap()
    }

    /// The components of `kernel` an agent's model is made of - built of the kind `model` with
    /// `params` - in a name given once.
    async fn model_of(kernel: &Kernel, params: Value) -> ComponentResult<ComponentContainer> {
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params,
                ..ComponentDescriptor::new("model", MODEL_KIND)
            }],
        };
        let facets = [TypeKey::of::<dyn ModelClient>(), TypeKey::of::<dyn ModelProfileProvider>()];
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        Arc::clone(kernel.component_registry()).build(&config, &facets, &context).await
    }

    /// What the model of `container` answers.
    async fn answer_of(container: &ComponentContainer) -> Option<String> {
        let client = container.one::<dyn ModelClient>().await.unwrap();
        let events: Vec<_> = client.chat_completion(ActorId::None, &[], &[]).await.collect().await;
        match events.last() {
            Some(Ok(MessageEvent::Complete(message))) => message.content.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// Issue 520: an agent's model is the kernel's model its component names, framed as the
    /// component says; whether it sees is the model's.
    #[tokio::test]
    async fn the_model_a_component_names_answers_framed_as_the_component_says() {
        let kernel = offering(
            &[
                ("a", json!({ "answer": "from a", "seeing": true })),
                ("b", json!({ "answer": "from b" })),
            ],
            &[],
        );
        let a = model_of(&kernel, json!({ "name": "a", "sender_format": "xml" })).await.unwrap();
        assert_eq!(answer_of(&a).await.as_deref(), Some("from a"));
        let profile = a.one::<dyn ModelProfileProvider>().await.unwrap().profile();
        assert_eq!(profile.sender_format, SenderFormat::Xml);
        assert!(profile.images, "as the model says");

        let b = model_of(&kernel, json!({ "name": "b" })).await.unwrap();
        assert_eq!(answer_of(&b).await.as_deref(), Some("from b"));
        let profile = b.one::<dyn ModelProfileProvider>().await.unwrap().profile();
        assert!(!profile.images);
        assert_eq!(profile.reasoning_history, ReasoningHistory::CurrentTurn);
        assert_eq!(profile.system_messages, SystemMessages::OnlyAtStart);
        assert_eq!(
            (profile.sender_format, profile.sender_style),
            (SenderFormat::Prefix, SenderStyle::Handle),
            "told who said what (issue 135)"
        );
        assert!(profile.message_times, "and when (issue 424)");
    }

    /// Issue 520: a component that names no model talks to the one there is; one that names a
    /// model not configured, or none among several, is not built.
    #[tokio::test]
    async fn a_model_not_named_is_the_one_there_is() {
        let one = offering(&[("only", json!({ "answer": "the one" }))], &[]);
        let model = model_of(&one, json!({})).await.unwrap();
        assert_eq!(answer_of(&model).await.as_deref(), Some("the one"));
        let refused = model_of(&one, json!({ "name": "other" })).await.unwrap_err().to_string();
        assert!(refused.contains("'name' names 'other', which is not configured"), "{refused}");

        let two =
            offering(&[("a", json!({ "answer": "a" })), ("b", json!({ "answer": "b" }))], &[]);
        let refused = model_of(&two, json!({})).await.unwrap_err().to_string();
        assert!(refused.contains("has to name one where there are several"), "{refused}");
    }

    /// Issue 57: the model is given the variant of the texts its component names, and a component
    /// that names one not configured is not built.
    #[tokio::test]
    async fn the_model_is_given_the_variant_of_the_texts_its_component_names() {
        let gemma = tempfile::tempdir().unwrap();
        let kernel = offering(
            &[("only", json!({ "answer": "ok" }))],
            &[("gemma4", gemma.path().to_path_buf())],
        );
        let profile_of = |params: Value| {
            let kernel = &kernel;
            async move {
                let model = model_of(kernel, params).await?;
                Ok::<_, ComponentError>(model.one::<dyn ModelProfileProvider>().await?.profile())
            }
        };
        assert_eq!(
            profile_of(json!({ "prompt_texts": "gemma4" })).await.unwrap().prompt_texts,
            Some(PromptVariant::new("gemma4").unwrap())
        );
        assert_eq!(
            profile_of(json!({})).await.unwrap().prompt_texts,
            None,
            "the default unless configured"
        );
        let refused =
            profile_of(json!({ "prompt_texts": "llama" })).await.expect_err("not configured");
        assert!(refused.to_string().contains("no variant 'llama'"), "{refused}");
        assert!(
            profile_of(json!({ "prompt_texts": "Gemma 4" })).await.is_err(),
            "no name of a variant"
        );
    }
}
