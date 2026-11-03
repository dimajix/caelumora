//! The kind `language_model` (issue 520): an agent's language model, one of those the platform
//! offers, as the agent is given it - its client, which is built for this agent alone, and the
//! profile by which its context is framed for that model.
//!
//! Parameters: `name`, the model of `plugins.model.language` - left out, the one there is -,
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

use crate::manager::ModelManager;
use caelumora_api::component::{ComponentFacets, ComponentKind, Params};
use caelumora_api::model::{LanguageModelClient, ModelProfileProvider};
use caelumora_domain::context::{
    message_times_by_default, ModelProfile, ReasoningHistory, SenderFormat, SenderStyle,
    SystemMessages,
};
use caelumora_domain::prompt_catalogue::{PromptCatalogue, PromptVariant};
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::kernel::Acting;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of an agent's model: the default of both its facets.
const LANGUAGE_MODEL_KIND: &str = "language_model";

/// The parameters of a `language_model` component.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageModelParams {
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

impl Params for LanguageModelParams {}

/// How the context of the agent's model is framed, as its component is configured.
#[derive(Debug)]
struct Framing(ModelProfile);

impl ModelProfileProvider for Framing {
    fn profile(&self) -> ModelProfile {
        self.0
    }
}

/// Registers the kind `language_model` with `registry`, and the model's facets,
/// `LanguageModelClient` and `ModelProfileProvider`, as facets of which an agent has exactly one,
/// with it as their default.
pub(crate) fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn LanguageModelClient>(Some(LANGUAGE_MODEL_KIND));
    registry.single::<dyn ModelProfileProvider>(Some(LANGUAGE_MODEL_KIND));
    registry.register_async(
        ComponentKind::new(LANGUAGE_MODEL_KIND)
            .params::<LanguageModelParams>()
            .provides::<dyn LanguageModelClient>()
            .provides::<dyn ModelProfileProvider>()
            .uses::<ModelManager>()
            .uses::<Acting>()
            .uses::<PromptCatalogue>(),
        |descriptor, context| async move {
            let params = LanguageModelParams::read(&descriptor.params)?;
            context.service::<PromptCatalogue>()?.require_variant(params.prompt_texts)?;
            // For the agent its components are built for, in its name at each call.
            let acting = Acting::clone(context.service::<Acting>()?);
            let models = context.service::<ModelManager>()?;
            let client = models.language_client_for(acting, params.name.as_deref())?;
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
            facets.insert::<dyn LanguageModelClient>(Arc::new(client));
            facets.insert::<dyn ModelProfileProvider>(Arc::new(Framing(profile)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::LANGUAGE;
    use crate::testing::{configuring, kernel_of, ANSWERING};
    use caelumora_api::component::{ComponentContainer, ComponentError, ComponentResult};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::config::{AppConfig, BackendConfig};
    use caelumora_domain::message::MessageEvent;
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_security::SecurityContext;
    use futures::stream::StreamExt;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The kernel of a test that offers the language models `models`, each answering with its
    /// `answer`, and the variants of the texts `prompts`.
    fn offering(models: &[(&str, Value)], prompts: &[(&str, PathBuf)]) -> Kernel {
        let models: BTreeMap<&str, BackendConfig> = models
            .iter()
            .map(|(name, params)| (*name, BackendConfig::new(ANSWERING, params.clone())))
            .collect();
        let prompts =
            prompts.iter().map(|(variant, dir)| (variant.to_string(), dir.clone())).collect();
        kernel_of(AppConfig { prompts, ..configuring(json!({ LANGUAGE: models })) }).unwrap()
    }

    /// The components of `kernel` an agent's model is made of - built of the kind `language_model`
    /// with `params` - in a name given once.
    async fn model_of(kernel: &Kernel, params: Value) -> ComponentResult<ComponentContainer> {
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params,
                ..ComponentDescriptor::new("model", LANGUAGE_MODEL_KIND)
            }],
        };
        let facets =
            [TypeKey::of::<dyn LanguageModelClient>(), TypeKey::of::<dyn ModelProfileProvider>()];
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        Arc::clone(kernel.component_registry()).build(&config, &facets, &context).await
    }

    /// What the model of `container` answers.
    async fn answer_of(container: &ComponentContainer) -> Option<String> {
        let client = container.one::<dyn LanguageModelClient>().await.unwrap();
        let events: Vec<_> = client.chat_completion(ActorId::None, &[], &[]).await.collect().await;
        match events.last() {
            Some(Ok(MessageEvent::Complete(message))) => message.content.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// Issue 520: an agent's model is the platform's model its component names, framed as the
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
    /// model not configured, or none among several, or where none is configured, is not built.
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

        let none = offering(&[], &[]);
        let refused = model_of(&none, json!({})).await.unwrap_err().to_string();
        assert!(refused.contains("no language model is configured"), "{refused}");
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

    /// Issue 520: an agent that names no model is given the plugin's kind `language_model`, in
    /// place of whatever a test's kernel, or a program, declared before the plugin registered.
    #[tokio::test]
    async fn the_kind_language_model_is_the_default_of_the_model_s_facets() {
        let kernel = offering(&[("only", json!({ "answer": "the one" }))], &[]);
        let facets =
            [TypeKey::of::<dyn LanguageModelClient>(), TypeKey::of::<dyn ModelProfileProvider>()];
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        let registry = Arc::clone(kernel.component_registry());
        let container =
            registry.build(&ComponentConfig::default(), &facets, &context).await.unwrap();
        assert_eq!(answer_of(&container).await.as_deref(), Some("the one"));
    }
}
