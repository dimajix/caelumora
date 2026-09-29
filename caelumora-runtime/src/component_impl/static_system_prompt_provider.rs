//! The `static_system_prompt_provider` component: a fixed prompt part, the
//! system prompt by default (parameters `prompt` and `position`).

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{Prompt, PromptPosition, PromptProvider};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub const STATIC_SYSTEM_PROMPT_PROVIDER_KIND: &str = "static_system_prompt_provider";

#[derive(Debug)]
pub struct StaticSystemPromptProvider {
    pub prompt: String,
    pub position: PromptPosition,
}

#[async_trait]
impl PromptProvider for StaticSystemPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }
    async fn get_prompt(&self, _turn: &Turn) -> Result<Prompt> {
        Ok(self.prompt.clone().into())
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(STATIC_SYSTEM_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        |descriptor, _context| {
            let prompt = descriptor.params.get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| ComponentError::InvalidConfig("parameter 'prompt' is missing".to_string()))?;
            let position = match descriptor.params.get("position") {
                None => PromptPosition::default(),
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|_| ComponentError::InvalidConfig("parameter 'position' must be \"start\" or \"before_turn_input\"".to_string()))?,
            };

            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(StaticSystemPromptProvider {
                prompt,
                position,
            }));
            Ok(facets)
        },
    );
}
