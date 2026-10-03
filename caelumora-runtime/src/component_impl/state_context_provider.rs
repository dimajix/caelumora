//! The `state_context_provider` component: the agent's stored context with
//! the prompt parts of every `PromptProvider` of the agent (issues 47, 99).

use crate::agent_turn::Turn;
use caelumora_model::context::PromptPart;
use crate::component::{ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry};
use caelumora_model::component::ComponentFailPolicy;
use tracing::warn;
use crate::context::{ContextProvider, PromptPosition, PromptProvider};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::MessageRole;
use std::sync::Arc;

pub const STATE_CONTEXT_PROVIDER_KIND: &str = "state_context_provider";

/// The stored context with the prompt parts of `prompt_providers`: those at
/// `Start` joined into the system prompt before everything, those at
/// `BeforeTurnInput` joined into one user message before the newest message
/// of the turn. The default `ContextProvider` (issue 98).
#[derive(Debug)]
pub struct StateContextProvider {
    pub prompt_providers: Vec<ComponentImpl<dyn PromptProvider>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self, state: &mut Turn) -> Result<()> {
        let mut start = Vec::new();
        let mut before_input = Vec::new();
        for provider in &self.prompt_providers {
            // Each part as its component's fail policy says: a part that
            // fails open is left out of this turn, one that fails closed
            // fails the turn.
            let contribution = match provider.get_prompt(state).await {
                Ok(contribution) => contribution,
                Err(e) => match provider.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!("Prompt part of component '{}' left out: {:#}", provider.id, e);
                        continue;
                    }
                    ComponentFailPolicy::FailClosed => {
                        return Err(e.context(format!("the prompt part of component '{}' failed", provider.id)));
                    }
                },
            };
            let prompt = contribution.text.trim().to_string();
            if prompt.is_empty() {
                continue;
            }
            for reference in contribution.knowledge {
                state.record_knowledge(reference);
            }
            let part = (prompt, contribution.classification);
            match provider.position() {
                PromptPosition::Start => start.push(part),
                PromptPosition::BeforeTurnInput => before_input.push(part),
            }
        }

        // The system prompt before everything.
        let transcript = &state.agent_channel_state().transcript;
        let mut parts = Vec::with_capacity(2);
        if !start.is_empty() {
            let (text, classification) = joined(start);
            parts.push(PromptPart { role: MessageRole::System, text, after: None, classification });
        }
        if !before_input.is_empty() {
            // Before the newest message, which is the turn's input - unless
            // the agent wrote it itself, in a turn that has none (e.g. after
            // `/wakeup`): then at the end.
            let mut messages = transcript.entries().rev().filter_map(|n| n.entry.message().map(|m| (n.digest, m)));
            let own = ActorId::Agent(state.agent_id());
            let after = match messages.next() {
                Some((_, input)) if input.sender_id != own => messages.next().map(|(digest, _)| digest),
                Some((own_message, _)) => Some(own_message),
                None => None,
            };
            let (text, classification) = joined(before_input);
            parts.push(PromptPart { role: MessageRole::User, text, after, classification });
        }
        state.set_context(parts);

        Ok(())
    }
}

/// The prompt parts of one position joined into one: their texts, and the
/// join of their classifications - `None` if one of them is not known, or
/// they have no audience in common (issue 168).
fn joined(parts: Vec<(String, Option<Classification>)>) -> (String, Option<Classification>) {
    let (texts, classifications): (Vec<String>, Vec<Option<Classification>>) = parts.into_iter().unzip();
    let classification = classifications.into_iter().collect::<Option<Vec<_>>>().and_then(|c| Classification::join_all(&c));
    (texts.join("\n\n"), classification)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(STATE_CONTEXT_PROVIDER_KIND).provides::<dyn ContextProvider>(),
        |_descriptor, context| async move {
            // Every prompt part of the agent, in the order of its
            // configuration: the parts of one position are joined in this
            // order, and a part that changes its place changes the prompt -
            // and its cache - for nothing.
            let prompt_providers = context.all::<dyn PromptProvider>().await?;

            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ContextProvider>(Arc::new(StateContextProvider {
                prompt_providers
            }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::AgentStateHandle;
    use crate::component_impl::static_system_prompt_provider::StaticSystemPromptProvider;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::{Message, MessageId};

    fn message(content: &str) -> Message {
        Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    /// `provider` as a component with `fail_policy`.
    fn part(provider: Arc<dyn PromptProvider>, fail_policy: ComponentFailPolicy) -> ComponentImpl<dyn PromptProvider> {
        ComponentImpl { id: "part".to_string(), fail_policy, facet: provider }
    }

    /// A prompt part that cannot be had.
    #[derive(Debug)]
    struct Unavailable;

    #[async_trait::async_trait]
    impl PromptProvider for Unavailable {
        async fn get_prompt(&self, _turn: &Turn) -> anyhow::Result<crate::context::Prompt> {
            Err(anyhow::anyhow!("unavailable"))
        }
    }

    #[tokio::test]
    async fn a_failing_prompt_part_is_left_out_or_fails_the_turn_as_its_component_says() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut agent_channel_state, AgentStateHandle::detached());
        let brief = || part(Arc::new(StaticSystemPromptProvider { prompt: "Be brief.".to_string(), position: PromptPosition::Start, classification: Classification::PUBLIC }), ComponentFailPolicy::FailClosed);

        let open = StateContextProvider { prompt_providers: vec![part(Arc::new(Unavailable), ComponentFailPolicy::FailOpen), brief()] };
        open.current_context(&mut turn).await.expect("a part that fails open is left out");
        assert_eq!(turn.full_context().unwrap().0[0].content.as_deref(), Some("Be brief."), "the others are given");

        let closed = StateContextProvider { prompt_providers: vec![part(Arc::new(Unavailable), ComponentFailPolicy::FailClosed), brief()] };
        let error = closed.current_context(&mut turn).await.expect_err("a part that fails closed fails the turn");
        assert!(format!("{error:#}").contains("component 'part'"), "{error:#}");
    }

    #[tokio::test]
    async fn prompt_parts_go_where_their_position_says_and_only_the_conversation_is_stored() {
        let agent_id = AgentId::new();
        let mut agent_channel_state = AgentChannelState::default();
        let provider = StateContextProvider {
            prompt_providers: vec![
                part(Arc::new(StaticSystemPromptProvider { prompt: "Be brief.".to_string(), position: PromptPosition::Start, classification: Classification::PUBLIC }), ComponentFailPolicy::FailClosed),
                part(Arc::new(StaticSystemPromptProvider { prompt: "Notes: none.".to_string(), position: PromptPosition::BeforeTurnInput, classification: Classification::PUBLIC }), ComponentFailPolicy::FailClosed),
            ],
        };
        let contents = |turn: &Turn| turn.full_context().unwrap().0.iter()
            .map(|m| format!("{:?}:{}", m.role, m.content.clone().unwrap_or_default()))
            .collect::<Vec<_>>();

        let mut earlier = message("earlier");
        earlier.role = MessageRole::User;
        let mut question = message("question");
        question.role = MessageRole::User;
        agent_channel_state.perceive(Vec::new(), vec![earlier, question]);
        let mut turn = Turn::begun(agent_id, "agent".to_string(), &mut agent_channel_state, AgentStateHandle::detached());
        provider.current_context(&mut turn).await.unwrap();
        assert_eq!(contents(&turn), ["System:Be brief.", "User:earlier", "User:Notes: none.", "User:question"],
            "the system prompt first, the notes before the turn's input");
        assert_eq!(turn.agent_channel_state().transcript.messages().len(), 2, "prompt parts are never stored in the context");

        // A turn without new input - the newest message is the agent's own.
        let mut own = message("my answer");
        own.sender_id = ActorId::Agent(agent_id);
        agent_channel_state.perceive(Vec::new(), vec![own]);
        let mut turn = Turn::begun(agent_id, "agent".to_string(), &mut agent_channel_state, AgentStateHandle::detached());
        provider.current_context(&mut turn).await.unwrap();
        assert_eq!(contents(&turn).last().map(String::as_str), Some("User:Notes: none."), "then at the end");
    }
}
