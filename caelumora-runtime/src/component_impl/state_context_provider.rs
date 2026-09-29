//! The `state_context_provider` component: the agent's stored context with
//! the prompt parts of every `PromptProvider` of the agent (issues 47, 99).

use crate::agent_turn::Turn;
use caelumora_model::message::ContextItem;
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

        // The transcript, each entry with its digest for `set_context`.
        let mut conversation: Vec<ContextItem> = state.agent_channel_state().transcript.entries().iter()
            .map(|e| ContextItem::Conversation { message: e.message.clone(), digest: e.digest })
            .collect();
        if !before_input.is_empty() {
            // The newest message is the turn's input - unless the agent wrote
            // it itself, in a turn that has none (e.g. after `/wakeup`).
            let own = ActorId::Agent(state.agent_id);
            let at = match conversation.last() {
                Some(ContextItem::Conversation { message, .. }) if message.sender_id != own => conversation.len() - 1,
                _ => conversation.len(),
            };
            conversation.insert(at, ContextItem::prompt(MessageRole::User, before_input.join("\n\n")));
        }

        let mut items = Vec::with_capacity(conversation.len() + 1);
        if !start.is_empty() {
            items.push(ContextItem::prompt(MessageRole::System, start.join("\n\n")));
        }
        items.extend(conversation);
        state.set_context(items);

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
