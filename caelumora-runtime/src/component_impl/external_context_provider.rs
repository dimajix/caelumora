//! The `external_context_provider` component (issue 214): what an external
//! agent's remote agent is given besides what was said - the prompt parts of
//! every `PromptProvider` of its personality, such as who is in the channel
//! (issue 204). The default `ContextProvider` of an external agent.
//!
//! Unlike a turn's (`turn_context_provider`), it neither joins the parts nor
//! places them: the remote agent keeps its conversation, so its actor gives
//! each part only when it changed, and tells them apart by the component
//! they came from (`PromptPart::source`). Every part goes first, as text of
//! the remote agent's user; positions mean nothing here.

use crate::component::{ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry};
use crate::context::{ContextProvider, PromptProvider};
use crate::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::context::{KnowledgeRef, PromptPart};
use caelumora_model::message::MessageRole;
use std::sync::Arc;

pub const EXTERNAL_CONTEXT_PROVIDER_KIND: &str = "external_context_provider";

/// The prompt parts of `prompt_providers`, each on its own, with the
/// component it came from.
#[derive(Debug)]
pub struct ExternalContextProvider {
    pub prompt_providers: Vec<ComponentImpl<dyn PromptProvider>>,
}

#[async_trait]
impl ContextProvider for ExternalContextProvider {
    async fn current_context(&self, work: &AgentWork<'_>) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>)> {
        let mut parts = Vec::new();
        let mut knowledge = Vec::new();
        for provider in &self.prompt_providers {
            let Some(contribution) = provider.apply_fail_policy(provider.get_prompt(work).await)? else { continue };
            let text = contribution.text.trim();
            if text.is_empty() {
                continue;
            }
            knowledge.extend(contribution.knowledge);
            parts.push(PromptPart {
                role: MessageRole::User,
                text: text.to_string(),
                after: None,
                classification: contribution.classification,
                source: Some(provider.id.clone()),
            });
        }
        Ok((parts, knowledge))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(EXTERNAL_CONTEXT_PROVIDER_KIND).provides::<dyn ContextProvider>(),
        |_descriptor, context| async move {
            // In the order of the configuration, which is the order they are
            // given in.
            let prompt_providers = context.all::<dyn PromptProvider>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ContextProvider>(Arc::new(ExternalContextProvider { prompt_providers }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::AgentStateHandle;
    use crate::component_impl::static_system_prompt_provider::StaticSystemPromptProvider;
    use crate::context::PromptPosition;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::component::ComponentFailPolicy;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::work::Work;

    fn fixed(id: &str, prompt: &str, position: PromptPosition) -> ComponentImpl<dyn PromptProvider> {
        let provider = StaticSystemPromptProvider { prompt: prompt.to_string(), position, classification: Classification::PUBLIC };
        ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailClosed, facet: Arc::new(provider) }
    }

    #[tokio::test]
    async fn every_part_is_given_on_its_own_with_where_it_came_from_whatever_its_position() {
        let provider = ExternalContextProvider { prompt_providers: vec![
            fixed("frame", " Be brief. ", PromptPosition::BeforeTurnInput),
            fixed("empty", "  ", PromptPosition::Start),
            fixed("rules", "Answer in German.", PromptPosition::Start),
        ] };
        let (state, agent_state, work) = (AgentChannelState::default(), AgentStateHandle::detached(), Work::default());
        let (parts, knowledge) = provider.current_context(&AgentWork::new(AgentId::new(), &work, &state, &agent_state, None)).await.unwrap();

        let given: Vec<_> = parts.iter().map(|p| (p.source.as_deref(), p.text.as_str(), p.role, p.after)).collect();
        assert_eq!(given, [(Some("frame"), "Be brief.", MessageRole::User, None), (Some("rules"), "Answer in German.", MessageRole::User, None)]);
        assert!(knowledge.is_empty());
    }
}
