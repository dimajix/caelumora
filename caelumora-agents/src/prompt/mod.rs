//! What an agent's model is given (issue 47): the facets that build it - a
//! `ContextProvider` assembles it, `PromptProvider`s contribute prompt parts
//! -, what their components share, and the built-in components that provide
//! them alone, one file per component kind. An agent has exactly one
//! `ContextProvider` and any number of `PromptProvider`s (issue 98). A
//! component that offers a prompt part along with another facet - tools,
//! say - stays in `components`.
//!
//! What the parts say is the agents' namespace of the prompt catalogue
//! (`PROMPTS`, issue 57): a provider gets the catalogue when it is made and
//! renders its text from it.

pub mod channel_briefing_prompt_provider;
pub mod channel_language_prompt_provider;
pub mod channel_mentions_prompt_provider;
pub mod channel_participants_prompt_provider;
pub mod current_time_prompt_provider;
pub mod external_context_provider;
pub mod request_reason_prompt_provider;
pub mod static_system_prompt_provider;
pub mod tool_sets_prompt_provider;
pub mod turn_context_provider;

use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, ModelProfile, PromptPart};
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptError, PromptTexts};
use caelumora_model::store::StoreError;
use caelumora_runtime::component::{
    config_of_params, ComponentContext, ComponentImpl, ComponentKind, ComponentRegistry,
    ComponentResult,
};
use caelumora_runtime::view::ViewError;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;

/// The namespace of the agents' texts in the prompt catalogue.
pub const NAMESPACE: &str = "agents";

/// The agents' texts (issue 57), added to the catalogue under `NAMESPACE`
/// by `AgentsPlugin`.
pub const PROMPTS: &str = include_str!("../prompts.yaml");

/// Where a prompt part goes in the context (issue 47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptPosition {
    /// Before everything, as the system prompt. Many models put it first
    /// and cache what follows it, so it should rarely change.
    #[default]
    Start,
    /// Transiently before the newest message of the turn, never stored in
    /// the context - for what changes often, such as notes. Given as a user
    /// message: some models merge every system message into the system
    /// prompt and move it to the front, where it would invalidate the cache.
    BeforeTurnInput,
}

/// What a prompt provider that takes its position alone is configured
/// with: where its part goes, its kind's default if left out.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PositionConfig {
    #[serde(default)]
    position: Option<PromptPosition>,
}

impl PositionConfig {
    /// The position `params` configure, `default` - the kind's - if they
    /// name none.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if they are not its parameters.
    pub(crate) fn position_of(
        params: &Value,
        default: PromptPosition,
    ) -> ComponentResult<PromptPosition> {
        let Self { position } = config_of_params(params)?;
        Ok(position.unwrap_or(default))
    }
}

/// What a prompt provider that tells of its channel is configured with:
/// where its part goes, and whether it tells only what changed - for an
/// external agent, which keeps what it was told.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelPartConfig {
    #[serde(default)]
    pub(crate) position: PromptPosition,
    #[serde(default)]
    pub(crate) changes_only: bool,
}

/// What a [`PromptProvider`] contributes: the text of its part, the
/// knowledge from other places it contains, recorded with the turn's
/// messages (issue 17), and how what the text holds besides that knowledge
/// is classified (issue 168). The turn has seen both: what it writes is
/// classified at least so.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub text: String,
    pub knowledge: Vec<KnowledgeRef>,
    /// `None`: not known, counted as the channel - a part that does not
    /// say what it holds may hold anything the channel may.
    pub classification: Option<Classification>,
}

impl Prompt {
    /// `text`, which holds nothing but `knowledge` - each entry classified
    /// in its reference.
    pub fn of_knowledge(text: String, knowledge: Vec<KnowledgeRef>) -> Self {
        Self { text, knowledge, classification: Some(Classification::PUBLIC) }
    }

    /// `text`, holding no knowledge from elsewhere, classified
    /// `classification`.
    pub fn classified(text: String, classification: Classification) -> Self {
        Self { text, knowledge: Vec::new(), classification: Some(classification) }
    }
}

/// A text that does not say how it is classified: as the channel.
impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self { text, knowledge: Vec::new(), classification: None }
    }
}

/// Contributes a prompt part: the system prompt, or other text the model is
/// given besides the conversation. `TurnContextProvider` composes the parts
/// of every one of the agent's providers, by position, in the order of its
/// configuration.
#[async_trait]
pub trait PromptProvider: Send + Sync + Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    /// The part for `situation` - a turn about to start, or a request to an
    /// external agent about to be sent (issue 204); empty text for none.
    ///
    /// # Errors
    /// `ContextError` if the part cannot be had.
    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError>;
}

/// The part of the prompt provider `provider` for `situation`, as its fail
/// policy says: `None` if it failed and fails open. A function of the
/// prompt module rather than a method of `ComponentImpl`, whose methods are
/// the component registry's alone (issue 370), as `apply_stage_processors`
/// is the message stages'.
///
/// # Errors
/// `ContextError::Component`, naming the component, if it failed and
/// fails closed.
pub(crate) async fn prompt_for(
    provider: &ComponentImpl<dyn PromptProvider>,
    situation: &Situation<'_>,
) -> Result<Option<Prompt>, ContextError> {
    provider.apply_fail_policy(provider.get_prompt(situation).await).map_err(|cause| {
        ContextError::Component { component: provider.id.clone(), cause: Box::new(cause) }
    })
}

/// Assembles what the model of an agent is given besides the conversation,
/// once, at the start of its work (issue 214): the prompt parts, where they
/// go, and the knowledge from other places they hold. It only answers; the
/// caller takes in what it answered, both at once - a turn with
/// `Turn::set_context`, an external agent's work with
/// `Work::take_in_context` (issue 285). An agent has exactly one
/// (`ComponentRegistry::single`, issue 98): the component of its
/// personality that provides it, or the default - `turn_context_provider`,
/// unless the configuration names another (`AppConfig::component_defaults`).
/// Two would each answer for the whole, so a personality with two is
/// refused.
#[async_trait]
pub trait ContextProvider: Send + Sync + Debug {
    /// The prompt parts for `situation`, each where it goes, and the knowledge
    /// they hold.
    ///
    /// # Errors
    /// `ContextError` if what the model is to be given cannot be had - a
    /// prompt provider that fails closed.
    async fn current_context(
        &self,
        situation: &Situation<'_>,
    ) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>), ContextError>;
}

/// How an agent's model is to be given its context (issue 57): the profile
/// of the model - which reasoning of earlier messages it is given back, how
/// the senders of others' messages are marked, say (issues 63, 135). A
/// property of the model, fixed for as long as its component lives, kept in
/// the context of every turn and applied where the model's input is
/// resolved (`ContextResolver`). An agent has exactly one: its model's
/// component provides it beside its `LLMClient`, an external agent's beside
/// its `ExternalAgent` - so what writes for a model learns how without
/// asking the model itself. Without a default in the core, as the
/// `LLMClient`: the program names the kind of its model for both.
pub trait ModelProfileProvider: Send + Sync + Debug {
    fn profile(&self) -> ModelProfile;
}

/// `kind`, declaring what a component of it needs to write for the agent's
/// model (issue 57): the prompt catalogue, and the profile that names the
/// variant of its texts (`texts_of`).
pub fn writing_for_the_model(kind: ComponentKind) -> ComponentKind {
    kind.uses::<PromptCatalogue>().needs::<dyn ModelProfileProvider>()
}

/// The texts a component writes for the agent's model with, as it is
/// built (issue 57): the catalogue, in the variant the profile of the
/// model names. A running agent's profile does not change - its components
/// are built once, as its actor starts -, so a component keeps them. Its
/// kind declares what this takes (`writing_for_the_model`).
///
/// # Errors
/// What building the model's component reports; `ComponentError::NoService`
/// if the kind does not declare it uses the catalogue.
pub async fn texts_of(context: &ComponentContext) -> ComponentResult<PromptTexts> {
    let profile = context.one::<dyn ModelProfileProvider>().await?.profile();
    Ok(profile.texts(context.service::<PromptCatalogue>()?))
}

/// Why what an agent's model is to be given cannot be had (issue 336):
/// what a `ContextProvider` or a `PromptProvider` answers. A plugin's
/// provider translates its own errors into it, deciding there what may be
/// shown; `crate::error::user_message` judges each variant.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// A failure in words that may be shown to users.
    #[error("{0}")]
    Failed(String),
    /// What reading through the views reported, judged as view errors are.
    #[error("{0}")]
    View(#[from] ViewError),
    /// What the store reported, judged as store errors are.
    #[error("{0}")]
    Store(#[from] StoreError),
    /// A text of the prompt catalogue that cannot be had - a fault of the
    /// texts or of the provider, named by its key.
    #[error("{0}")]
    Prompt(#[from] PromptError),
    /// A prompt provider that fails closed failed with `cause`.
    #[error("component '{component}' failed")]
    Component {
        component: String,
        #[source]
        cause: Box<ContextError>,
    },
}

/// Registers the component kinds of this directory with `registry`, and the
/// `ContextProvider` as a facet of which an agent has exactly one, with its
/// default kind (issue 99).
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn ContextProvider>(Some(turn_context_provider::TURN_CONTEXT_PROVIDER_KIND));
    registry.single::<dyn ModelProfileProvider>(None);

    channel_briefing_prompt_provider::register(registry);
    channel_language_prompt_provider::register(registry);
    channel_mentions_prompt_provider::register(registry);
    channel_participants_prompt_provider::register(registry);
    current_time_prompt_provider::register(registry);
    external_context_provider::register(registry);
    request_reason_prompt_provider::register(registry);
    static_system_prompt_provider::register(registry);
    tool_sets_prompt_provider::register(registry);
    turn_context_provider::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LLMClient, LLMResult};
    use crate::prompt::current_time_prompt_provider::CURRENT_TIME_PROMPT_PROVIDER_KIND;
    use crate::testing::TestAgents;
    use crate::tool::calculator_tool::CALCULATOR_TOOL_KIND;
    use crate::tool::{denied_result, ToolProvider};
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::config::AppConfig;
    use caelumora_model::message::{Message, MessageEvent};
    use caelumora_model::prompt_catalogue::PromptVariant;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::SessionId;
    use caelumora_model::store::Persistence;
    use caelumora_model::tool::Tool;
    use caelumora_model::type_map::TypeKey;
    use caelumora_model::work::Work;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext, ComponentFacets};
    use caelumora_runtime::session::testing::detached_actor_runtime_in;
    use caelumora_runtime::testing::TestRuntime;
    use futures::stream;
    use futures::stream::BoxStream;
    use futures::StreamExt;
    use std::fs;
    use std::sync::Arc;

    /// A model whose profile names the variant `gemma4`, never asked.
    #[derive(Debug)]
    struct Gemma;

    #[async_trait]
    impl LLMClient for Gemma {
        async fn chat_completion(
            &self,
            _: &[Message],
            _: &[Tool],
        ) -> BoxStream<'static, LLMResult<MessageEvent>> {
            stream::empty().boxed()
        }
    }

    impl ModelProfileProvider for Gemma {
        fn profile(&self) -> ModelProfile {
            ModelProfile {
                prompt_texts: Some(PromptVariant::new("gemma4").unwrap()),
                ..ModelProfile::default()
            }
        }
    }

    /// Issue 57: what writes for a model takes its texts in the variant the
    /// model's profile names - a prompt part and a tool as they are built,
    /// what the work words itself by the profile of the work -, and falls
    /// back to the default for what the variant leaves out.
    #[tokio::test]
    async fn the_texts_are_given_in_the_variant_of_the_model() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("agents.yaml"), "current_time: |\n  [Now]\n  {{ time }}\ntools:\n  calculator_tool:\n    calculate: Computes.\nresults:\n  denied: Declined.\n").unwrap();
        let store = Persistence::in_memory();
        let components = |registry: &mut ComponentRegistry| {
            registry.register(
                ComponentKind::new("gemma")
                    .provides::<dyn LLMClient>()
                    .provides::<dyn ModelProfileProvider>(),
                |_, _| {
                    let model = Arc::new(Gemma);
                    let mut facets = ComponentFacets::empty();
                    facets.insert::<dyn LLMClient>(Arc::<Gemma>::clone(&model));
                    facets.insert::<dyn ModelProfileProvider>(model);
                    Ok(facets)
                },
            );
        };
        let config = AppConfig {
            prompts: [("gemma4".to_string(), dir.path().to_path_buf())].into(),
            ..AppConfig::default()
        };
        let runtime = TestRuntime {
            store,
            config,
            components: Box::new(components),
            plugins: vec![&TestAgents],
            ..Default::default()
        }
        .build();

        let components = ComponentConfig {
            component_nodes: vec![
                ComponentDescriptor::new("model", "gemma"),
                ComponentDescriptor::new("time", CURRENT_TIME_PROMPT_PROVIDER_KIND),
                ComponentDescriptor::new("calculator", CALCULATOR_TOOL_KIND),
            ],
        };
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        let built = ComponentContainer::from_config(
            &components,
            &[TypeKey::of::<dyn PromptProvider>(), TypeKey::of::<dyn ToolProvider>()],
            runtime.component_registry(),
            &context,
        )
        .await
        .unwrap();
        let calculator = built.get_all::<dyn ToolProvider>(None).unwrap().remove(0);
        assert_eq!(
            calculator.tools()[0].description,
            "Computes.",
            "the tool, in its model's variant"
        );
        assert_eq!(
            calculator.description(),
            "Evaluate mathematical expressions.",
            "what the variant leaves out, as the default"
        );

        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let actor_runtime = detached_actor_runtime_in(Arc::clone(&runtime), session, channel);
        let work = Work::default().given_as(Gemma.profile());
        let (state, agent_state) = (AgentChannelState::default(), AgentStateHandle::detached());
        let situation = Situation::new(AgentId::new(), &work, &state, &agent_state, &actor_runtime);
        let time = built.get_all::<dyn PromptProvider>(None).unwrap().remove(0);
        assert!(
            time.get_prompt(&situation).await.unwrap().text.starts_with("[Now]\n"),
            "the prompt part, too"
        );
        assert!(
            denied_result(&situation.texts()).contains("Declined."),
            "and what the work words itself, by its profile"
        );
        let elsewhere = Work::default();
        assert!(
            denied_result(
                &Situation::new(AgentId::new(), &elsewhere, &state, &agent_state, &actor_runtime)
                    .texts()
            )
            .contains("The user declined"),
            "work of another profile, as it"
        );
    }
}
