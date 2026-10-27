//! The `request_reason_prompt_provider` component (issue 439): tells an
//! agent the channel's responder policy chose - nobody mentioned it, nobody
//! nudged it - that it was only chosen, and that with nothing to add it
//! stays silent. Without it, a model takes every request to respond for an
//! order to answer, and greets back, agrees or writes that it has nothing
//! to say (`work/quiet_agents/RESULTS.md`, findings 3-5).
//!
//! An agent asked for any other reason is told nothing: told when to stay
//! silent, a mentioned agent judged the message that mentioned it rather
//! than what was its own to do, and stopped working where it was only
//! cheered on (findings 6, 7). It changes with every turn, so it goes
//! before the turn's input by default (parameter `position`), not into the
//! system prompt. A personality without the component leaves its agents to
//! answer whenever they are asked.

use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, texts_of, writing_for_the_model};
use crate::situation::Situation;
use crate::tool::stop_tool::SILENCE_TOOL_NAME;
use async_trait::async_trait;
use caelumora_model::actor::RequestReason;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptTexts};
use std::sync::Arc;

pub const REQUEST_REASON_PROMPT_PROVIDER_KIND: &str = "request_reason_prompt_provider";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.request_reason";

#[derive(Debug)]
pub struct RequestReasonPromptProvider {
    pub position: PromptPosition,
    pub prompts: PromptTexts,
}

#[async_trait]
impl PromptProvider for RequestReasonPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    /// Nothing unless the work answers a request of the policy's choice;
    /// then how to stay silent - by `silence` where the work is offered it.
    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        if situation.asked() != Some(RequestReason::Chosen) {
            return Ok(Prompt::classified(String::new(), Classification::PUBLIC));
        }
        let offered = situation.tool_sets().iter().flat_map(|set| &set.tools).any(|tool| tool.name == SILENCE_TOOL_NAME);
        let text = self.prompts.render(TEXT, context! { silence => offered.then_some(SILENCE_TOOL_NAME) })?;
        // Why someone is asked is no secret of anything.
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(REQUEST_REASON_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>()),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position = PromptPosition::of_params_or(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(RequestReasonPromptProvider { position, prompts: texts }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use crate::testing::detached_actor_runtime;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::tool::{Tool, ToolParameters, ToolSet, ToolVisibility};
    use caelumora_model::work::Work;

    /// What the provider tells in work asked for as `asked`, offered the
    /// tools named `tools`.
    async fn told(asked: Option<RequestReason>, tools: &[&str]) -> String {
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(Default::default(), asked, Default::default(), Classification::CHANNEL_DEFAULT, Default::default()));
        let tools = tools.iter().map(|name| Tool {
            name: name.to_string(),
            description: String::new(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: Default::default(),
        }).collect();
        state.offer_tools(vec![ToolSet::new("stop", "", tools)]);
        let work: Work = state.turn.as_ref().map(|turn| turn.work.clone()).expect("begun");
        let provider = RequestReasonPromptProvider { position: PromptPosition::BeforeTurnInput, prompts: testing::prompts() };
        let prompt = provider.get_prompt(&Situation::new(AgentId::new(), &work, &state, &AgentStateHandle::detached(), &detached_actor_runtime())).await.unwrap();
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));
        prompt.text
    }

    #[tokio::test]
    async fn an_agent_only_chosen_is_told_so_and_how_to_stay_silent() {
        let told_chosen = told(Some(RequestReason::Chosen), &["stop", SILENCE_TOOL_NAME]).await;
        assert!(told_chosen.starts_with("[Why you are asked to respond]\nNobody mentioned you."), "{told_chosen}");
        assert!(told_chosen.ends_with("waiting - call `silence` and write nothing.\n"), "by the tool it is offered: {told_chosen}");
        let without_silence = told(Some(RequestReason::Chosen), &["stop"]).await;
        assert!(without_silence.ends_with("waiting - write nothing.\n"), "by writing nothing where it is not: {without_silence}");
    }

    #[tokio::test]
    async fn an_agent_asked_for_any_other_reason_is_told_nothing() {
        for asked in [Some(RequestReason::Undecided), Some(RequestReason::Mentioned), Some(RequestReason::Nudged), None] {
            assert_eq!(told(asked, &[SILENCE_TOOL_NAME]).await, "", "{asked:?}");
        }
    }
}
