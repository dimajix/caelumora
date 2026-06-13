use anyhow::Result;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use caelumora_model::agent::AgentState;
use caelumora_model::message::Message;
use crate::component::{
    ComponentFacets, ComponentRegistry, ContextProvider, MessageProcessor, PerceiveInProcessor,
    SystemPromptProvider, TurnContext, STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND,
};
use caelumora_model::message::MessageRole;

#[derive(Debug)]
pub struct StateContextAppender {
    pub state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl MessageProcessor for StateContextAppender {
    fn name(&self) -> &str {
        STATE_CONTEXT_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, _context: &mut TurnContext) -> Result<Vec<Message>> {
        let mut state_guard = self.state.lock().await;
        state_guard.context.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateContextAppender {}

#[derive(Debug)]
pub struct StateContextProvider {
    pub state: Arc<Mutex<AgentState>>,
    pub prompt_providers: Vec<Arc<dyn SystemPromptProvider>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self) -> Result<Vec<Message>> {
        let mut full_context = Vec::new();

        // 1. Assemble System Prompt from all providers
        let mut system_prompts = Vec::new();
        for provider in &self.prompt_providers {
            system_prompts.push(provider.get_prompt().await?);
        }

        if !system_prompts.is_empty() {
            full_context.push(Message {
                role: MessageRole::System,
                content: Some(system_prompts.join("\n\n")),
                ..Default::default()
            });
        }

        // 2. Append history/compacted messages from state
        let state_guard = self.state.lock().await;
        full_context.extend(state_guard.context.messages());

        Ok(full_context)
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_CONTEXT_APPENDER_KIND,
        Arc::new(|descriptor| {
            let state = descriptor
                .state
                .ok_or_else(|| anyhow::anyhow!("Component '{}' requires agent state", descriptor.id))?;
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateContextAppender { state }));
            Ok(capabilities)
        }),
    );

    registry.register(
        STATE_CONTEXT_PROVIDER_KIND,
        Arc::new(|descriptor| {
            let state = descriptor
                .state
                .ok_or_else(|| anyhow::anyhow!("Component '{}' requires agent state", descriptor.id))?;

            let prompt_providers = descriptor.deps.values()
                .filter_map(|facets| facets.get::<dyn SystemPromptProvider>())
                .collect();

            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn ContextProvider>(Arc::new(StateContextProvider {
                state,
                prompt_providers
            }));
            Ok(capabilities)
        }),
    );
}
