//! The `external_context` component (issue 214): what an external
//! agent's remote agent is given besides what was said - the prompt parts of
//! every `PromptProvider` of its personality, such as who is in the channel
//! (issue 204). The default `ContextProvider` of an external agent.
//!
//! Unlike a turn's (`turn_context`), it neither joins the parts nor
//! places them: the remote agent keeps its conversation, so its actor gives
//! each part only when it changed, and tells them apart by the component
//! they came from (`Placement`). Every part goes first, as text of
//! the remote agent's user; positions mean nothing here. A component whose
//! part the remote agent was given and that has none now gives its
//! withdrawal in its place (`PromptProvider::withdrawn`): what the remote
//! agent keeps would hold otherwise.

use crate::prompt::{prompt_for, withdrawal_for, ContextError, ContextProvider, PromptProvider};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::ComponentImpl;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, Placement, PromptAt, PromptPart};
use caelumora_domain::message::MessageRole;
use std::sync::Arc;

pub const EXTERNAL_CONTEXT_KIND: &str = "external_context";

/// The prompt parts of `prompt_providers`, each on its own, with the
/// component it came from - its withdrawal for a part the remote agent was
/// given and that is empty now.
#[derive(Debug)]
pub struct ExternalContextProvider {
    pub prompt_providers: Vec<ComponentImpl<dyn PromptProvider>>,
}

#[async_trait]
impl ContextProvider for ExternalContextProvider {
    async fn current_context(
        &self,
        situation: &Situation<'_>,
    ) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>), ContextError> {
        let mut parts = Vec::new();
        let mut knowledge = Vec::new();
        for provider in &self.prompt_providers {
            let Some(contribution) = prompt_for(provider, situation).await? else { continue };
            let text = contribution.text.trim();
            if !text.is_empty() {
                knowledge.extend(contribution.knowledge);
                parts.push(part(provider, text, contribution.classification));
            } else if situation.agent_channel_state().remote.has_given_any(&provider.id) {
                if let Some(withdrawn) = withdrawal_for(provider)? {
                    parts.push(part(provider, withdrawn.trim(), Some(Classification::PUBLIC)));
                }
            }
        }
        Ok((parts, knowledge))
    }
}

/// The part `text` of `provider`, classified `classification`.
fn part(
    provider: &ComponentImpl<dyn PromptProvider>,
    text: &str,
    classification: Option<Classification>,
) -> PromptPart {
    PromptPart {
        role: MessageRole::User,
        text: text.to_string(),
        placement: Placement::Frame { at: PromptAt::Start, component: Some(provider.id.clone()) },
        classification,
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        ComponentKind::new(EXTERNAL_CONTEXT_KIND)
            .provides::<dyn ContextProvider>()
            .needs::<dyn PromptProvider>(),
        |_descriptor, context| async move {
            // In the order of the configuration, which is the order they are
            // given in.
            let prompt_providers = context.all::<dyn PromptProvider>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ContextProvider>(Arc::new(ExternalContextProvider {
                prompt_providers,
            }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::system_prompt::SystemPromptComponent;
    use crate::prompt::Prompt;
    use crate::prompt::PromptPosition;
    use crate::testing::detached_actor_runtime;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::component::ComponentFailPolicy;
    use caelumora_domain::digest;
    use caelumora_domain::work::Work;

    fn fixed(
        id: &str,
        prompt: &str,
        position: PromptPosition,
    ) -> ComponentImpl<dyn PromptProvider> {
        let provider = SystemPromptComponent {
            prompt: prompt.to_string(),
            position,
            classification: Classification::PUBLIC,
        };
        ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailClosed,
            facet: Arc::new(provider),
        }
    }

    #[tokio::test]
    async fn every_part_is_given_on_its_own_with_where_it_came_from_whatever_its_position() {
        let provider = ExternalContextProvider {
            prompt_providers: vec![
                fixed("frame", " Be brief. ", PromptPosition::BeforeTurnInput),
                fixed("empty", "  ", PromptPosition::Start),
                fixed("rules", "Answer in German.", PromptPosition::Start),
            ],
        };
        let state = AgentChannelState::default();
        let work = Work::default();
        let (parts, knowledge) = provider
            .current_context(&Situation::new(
                &work,
                &state,
                &*detached_actor_runtime(AgentId::new()),
            ))
            .await
            .unwrap();

        let given: Vec<_> =
            parts.iter().map(|p| (p.text.as_str(), p.role, p.placement.clone())).collect();
        let of = |component: &str| Placement::Frame {
            at: PromptAt::Start,
            component: Some(component.to_string()),
        };
        assert_eq!(
            given,
            [
                ("Be brief.", MessageRole::User, of("frame")),
                ("Answer in German.", MessageRole::User, of("rules"))
            ]
        );
        assert!(knowledge.is_empty());
    }

    /// A part that holds nothing, and its withdrawal.
    #[derive(Debug)]
    struct Withdrawing;

    #[async_trait]
    impl PromptProvider for Withdrawing {
        async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
            Ok(Prompt::classified(String::new(), Classification::PUBLIC))
        }

        fn withdrawn(&self) -> Result<Option<String>, ContextError> {
            Ok(Some("[Rules]\nNo longer.".to_string()))
        }
    }

    /// The parts `provider` gives an agent in `state`.
    async fn given(
        provider: &ExternalContextProvider,
        state: &AgentChannelState,
    ) -> Vec<PromptPart> {
        let work = Work::default();
        let runtime = detached_actor_runtime(AgentId::new());
        provider.current_context(&Situation::new(&work, state, &*runtime)).await.unwrap().0
    }

    #[tokio::test]
    async fn a_part_given_before_that_holds_nothing_now_is_withdrawn() {
        let provider = ExternalContextProvider {
            prompt_providers: vec![ComponentImpl {
                id: "rules".to_string(),
                fail_policy: ComponentFailPolicy::FailClosed,
                facet: Arc::new(Withdrawing),
            }],
        };
        let mut state = AgentChannelState::default();
        assert!(given(&provider, &state).await.is_empty(), "never given, so nothing to take back");
        state.remote.give("rules".to_string(), digest::prompt("[Rules]\nBe brief."));
        let parts = given(&provider, &state).await;
        let withdrawn: Vec<_> = parts
            .iter()
            .map(|p| (p.placement.clone(), p.text.as_str(), p.classification.clone()))
            .collect();
        let rules = Placement::Frame { at: PromptAt::Start, component: Some("rules".to_string()) };
        assert_eq!(
            withdrawn,
            [(rules, "[Rules]\nNo longer.", Some(Classification::PUBLIC))],
            "given before, so taken back"
        );
    }
}
