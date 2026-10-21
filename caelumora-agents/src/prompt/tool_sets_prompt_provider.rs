//! The `tool_sets_prompt_provider` component (issues 32, 194): tells the
//! agent's model which of its tool sets are unavailable, and why in general
//! terms. The model is given its tools natively, so it needs no prompt to
//! learn of them - but a source whose tools are missing leaves no trace
//! there, and the model would answer as if it could not do what it normally
//! can.
//!
//! Optional: a personality that does not configure it tells its model
//! nothing of missing tools. It reads the tool sets the turn began with, so
//! that what it says matches what the model is offered; with all of them
//! available it contributes nothing.

use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptCatalogue};
use caelumora_model::tool::ToolSet;
use std::sync::Arc;

pub const TOOL_SETS_PROMPT_PROVIDER_KIND: &str = "tool_sets_prompt_provider";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.tool_sets";

#[derive(Debug)]
pub struct ToolSetsPromptProvider {
    pub position: PromptPosition,
    pub prompts: Arc<PromptCatalogue>,
}

#[async_trait]
impl PromptProvider for ToolSetsPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // What the agent is configured with, nothing read from a channel.
        Ok(Prompt::classified(self.render(situation.tool_sets())?, Classification::PUBLIC))
    }
}

impl ToolSetsPromptProvider {
    /// The text naming the unavailable sets among `sets`, empty if there
    /// are none.
    fn render(&self, sets: &[ToolSet]) -> Result<String, ContextError> {
        let unavailable: Vec<_> = sets.iter()
            .filter_map(|set| set.unavailable.as_deref().map(|why| context! { name => set.name, description => one_line(&set.description), why => one_line(why) }))
            .collect();
        if unavailable.is_empty() {
            return Ok(String::new());
        }
        Ok(self.prompts.render(TEXT, context! { unavailable })?)
    }
}

/// `text` on one line, so that it cannot break out of its entry.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(TOOL_SETS_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>().uses::<PromptCatalogue>(),
        |descriptor, context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(ToolSetsPromptProvider { position, prompts: Arc::clone(context.service::<PromptCatalogue>()?) }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::turn::Turn;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::tool::{Tool, ToolParameters};
    use caelumora_model::work::Work;

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: "does something".to_string(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }
    }

    /// What the provider says in a turn begun with `sets`.
    async fn told(sets: Vec<ToolSet>) -> Prompt {
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(Default::default(), None, Default::default(), Classification::CHANNEL_DEFAULT, Default::default()));
        state.offer_tools(sets);
        let turn = Turn::under_way(AgentId::new(), &mut state);
        let provider = ToolSetsPromptProvider { position: PromptPosition::Start, prompts: testing::prompts() };
        provider.get_prompt(&turn.situation()).await.unwrap()
    }

    #[tokio::test]
    async fn the_unavailable_tool_sets_are_named_with_what_they_are_for_and_why() {
        let github = ToolSet {
            unavailable: Some("its server\ncannot be reached".to_string()),
            ..ToolSet::new("github-mcp", "The tools of\nthe issue tracker.", Vec::new())
        };
        let prompt = told(vec![ToolSet::new("calculator", "Evaluate expressions.", vec![tool("calculate")]), github]).await;
        assert!(prompt.text.contains("- github-mcp (The tools of the issue tracker.): its server cannot be reached\n"), "{}", prompt.text);
        assert!(!prompt.text.contains("calculator"), "only the unavailable ones: {}", prompt.text);
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));
    }

    #[tokio::test]
    async fn with_every_tool_set_available_it_says_nothing() {
        assert!(told(vec![ToolSet::new("calculator", "Evaluate expressions.", vec![tool("calculate")])]).await.text.is_empty());
        assert!(told(Vec::new()).await.text.is_empty(), "nor without tools");
    }
}
