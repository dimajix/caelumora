use crate::actor::TurnContext;
use crate::component::{ComponentError, ComponentFacets, ComponentRegistry};
use crate::component_impl::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::message::Message;
use caelumora_model::message::MessageRole;
use std::sync::Arc;

pub const STATE_CONTEXT_APPENDER_KIND: &str = "state_context_appender";
pub const STATE_CONTEXT_PROVIDER_KIND: &str = "state_context_provider";
pub const STATIC_SYSTEM_PROMPT_PROVIDER_KIND: &str = "static_system_prompt_provider";

#[async_trait]
pub trait SystemPromptProvider: Send + Sync + std::fmt::Debug {
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
pub struct StaticSystemPromptProvider {
    pub prompt: String,
}

#[async_trait]
impl SystemPromptProvider for StaticSystemPromptProvider {
    async fn get_prompt(&self) -> Result<String> {
        Ok(self.prompt.clone())
    }
}

#[derive(Debug)]
pub struct StateContextProvider {
    pub prompt_providers: Vec<Arc<dyn SystemPromptProvider>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self, state: &mut TurnContext) -> Result<()> {
        let mut full_context = Vec::new();

        // Assemble System Prompt from all providers
        let mut system_prompts = Vec::new();
        for provider in &self.prompt_providers {
            let p = provider.get_prompt().await?;
            let pt = p.trim().to_string();
            if !pt.is_empty() {
                system_prompts.push(pt);
            }
        }

        if !system_prompts.is_empty() {
            full_context.push(Message {
                role: MessageRole::System,
                content: Some(system_prompts.join("\n\n")),
                sender_id: ActorId::Agent(state.agent_id),
                sender_name: state.agent_name.clone(),
                ..Default::default()
            });
        }

        // 2. Append history/compacted messages from state
        full_context.extend(state.state_context());
        state.set_context(full_context);

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
                .filter_map(|facets| facets.get::<dyn SystemPromptProvider>())
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

            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn SystemPromptProvider>(Arc::new(StaticSystemPromptProvider {
                prompt
            }));
            Ok(capabilities)
        }),
    );
}
