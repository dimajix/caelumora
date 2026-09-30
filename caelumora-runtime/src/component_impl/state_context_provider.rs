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
            let contribution = match provider.facet.get_prompt(state).await {
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
            match provider.facet.position() {
                PromptPosition::Start => start.push(prompt),
                PromptPosition::BeforeTurnInput => before_input.push(prompt),
            }
        }

        // The system prompt before everything.
        let transcript = &state.agent_channel_state().transcript;
        let mut parts = Vec::with_capacity(2);
        if !start.is_empty() {
            parts.push(PromptPart { role: MessageRole::System, text: start.join("\n\n"), after: None });
        }
        if !before_input.is_empty() {
            // Before the newest message, which is the turn's input - unless
            // the agent wrote it itself, in a turn that has none (e.g. after
            // `/wakeup`): then at the end.
            let entries = transcript.entries();
            let own = ActorId::Agent(state.agent_id());
            let after = match entries.split_last() {
                Some((input, earlier)) if input.message.sender_id != own => earlier.last().map(|e| e.digest),
                Some((own_message, _)) => Some(own_message.digest),
                None => None,
            };
            parts.push(PromptPart { role: MessageRole::User, text: before_input.join("\n\n"), after });
        }
        state.set_context(parts);

        Ok(())
    }
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
