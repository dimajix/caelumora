use crate::actor::{ContextItem, TurnContext};
use crate::component::{ComponentError, ComponentFacets, ComponentRegistry};
use crate::component_impl::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::message::{Message, MessageRole};
use serde::Deserialize;
use std::sync::Arc;

pub const STATE_CONTEXT_APPENDER_KIND: &str = "state_context_appender";
pub const STATE_CONTEXT_PROVIDER_KIND: &str = "state_context_provider";
pub const STATIC_SYSTEM_PROMPT_PROVIDER_KIND: &str = "static_system_prompt_provider";

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

/// Contributes a prompt part: the system prompt, or other text the model is
/// given besides the conversation. `StateContextProvider` composes the parts
/// of the providers it depends on, by position.
#[async_trait]
pub trait PromptProvider: Send + Sync + std::fmt::Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    async fn get_prompt(&self) -> Result<String>;
}

#[async_trait]
pub trait ContextProvider: Send + Sync + std::fmt::Debug {
    async fn current_context(&self, state: &mut TurnContext) -> Result<()>;
}

#[derive(Debug)]
pub struct StateContextAppender {
}

#[async_trait]
impl MessageProcessor for StateContextAppender {
    fn name(&self) -> &str {
        STATE_CONTEXT_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, context: &mut TurnContext) -> Result<Vec<Message>> {
        context.extend_state_context(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateContextAppender {}

#[derive(Debug)]
pub struct StaticPromptProvider {
    pub prompt: String,
    pub position: PromptPosition,
}

#[async_trait]
impl PromptProvider for StaticPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }
    async fn get_prompt(&self) -> Result<String> {
        Ok(self.prompt.clone())
    }
}

/// The stored context with the prompt parts of `prompt_providers`: those at
/// `Start` joined into the system prompt before everything, those at
/// `BeforeTurnInput` joined into one user message before the newest message
/// of the turn.
#[derive(Debug)]
pub struct StateContextProvider {
    pub prompt_providers: Vec<Arc<dyn PromptProvider>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self, state: &mut TurnContext) -> Result<()> {
        let mut start = Vec::new();
        let mut before_input = Vec::new();
        for provider in &self.prompt_providers {
            let prompt = provider.get_prompt().await?.trim().to_string();
            if prompt.is_empty() {
                continue;
            }
            match provider.position() {
                PromptPosition::Start => start.push(prompt),
                PromptPosition::BeforeTurnInput => before_input.push(prompt),
            }
        }

        let mut conversation = state.stored_context();
        if !before_input.is_empty() {
            // The newest message is the turn's input - unless the agent wrote
            // it itself, in a turn that has none (e.g. after `/wakeup`).
            let own = ActorId::Agent(state.agent_id);
            let at = match conversation.last() {
                Some(ContextItem::Conversation { message, .. }) if message.sender_id != own => conversation.len() - 1,
                _ => conversation.len(),
            };
            conversation.insert(at, ContextItem::Prompt { role: MessageRole::User, text: before_input.join("\n\n") });
        }

        let mut items = Vec::with_capacity(conversation.len() + 1);
        if !start.is_empty() {
            items.push(ContextItem::Prompt { role: MessageRole::System, text: start.join("\n\n") });
        }
        items.extend(conversation);
        state.set_context(items);

        Ok(())
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_CONTEXT_APPENDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateContextAppender { }));
            Ok(capabilities)
        }),
    );

    registry.register(
        STATE_CONTEXT_PROVIDER_KIND,
        Box::new(|_descriptor, context| {
            let prompt_providers = context.deps.values()
                .filter_map(|facets| facets.get::<dyn PromptProvider>())
                .collect();

            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn ContextProvider>(Arc::new(StateContextProvider {
                prompt_providers
            }));
            Ok(capabilities)
        }),
    );

    registry.register(
        STATIC_SYSTEM_PROMPT_PROVIDER_KIND,
        Box::new(|descriptor, _context| {
            let prompt = descriptor.params.get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| ComponentError::InvalidConfig("parameter 'prompt' is missing".to_string()))?;
            let position = match descriptor.params.get("position") {
                None => PromptPosition::default(),
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|_| ComponentError::InvalidConfig("parameter 'position' must be \"start\" or \"before_turn_input\"".to_string()))?,
            };

            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PromptProvider>(Arc::new(StaticPromptProvider {
                prompt,
                position,
            }));
            Ok(capabilities)
        }),
    );
}
