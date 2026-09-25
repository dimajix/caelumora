//! The `static_system_prompt_provider` component: a fixed prompt part, the
//! system prompt by default (parameters `prompt` and `position`).

use crate::actor::TurnContext;
use crate::component::{ComponentError, ComponentFacets, ComponentRegistry};
use crate::component_impl::context::{Prompt, PromptPosition, PromptProvider};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub const STATIC_SYSTEM_PROMPT_PROVIDER_KIND: &str = "static_system_prompt_provider";

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
    async fn get_prompt(&self, _context: &TurnContext) -> Result<Prompt> {
        Ok(self.prompt.clone().into())
    }
}

pub fn register(registry: &mut ComponentRegistry) {
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
