//! The `static_system_prompt_provider` component: a fixed prompt part, the
//! system prompt by default (parameters `prompt`, `position` and
//! `classification`).

use crate::component::{ComponentError, ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{Prompt, PromptPosition, PromptProvider};
use crate::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use std::sync::Arc;

pub const STATIC_SYSTEM_PROMPT_PROVIDER_KIND: &str = "static_system_prompt_provider";

#[derive(Debug)]
pub struct StaticSystemPromptProvider {
    pub prompt: String,
    pub position: PromptPosition,
    /// How the prompt is classified (issue 168): public unless configured
    /// otherwise - who writes something confidential into a personality's
    /// prompt classifies it.
    pub classification: Classification,
}

#[async_trait]
impl PromptProvider for StaticSystemPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }
    async fn get_prompt(&self, _work: &AgentWork<'_>) -> Result<Prompt> {
        Ok(Prompt::classified(self.prompt.clone(), self.classification.clone()))
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

            let classification = match descriptor.params.get("classification") {
                None => Classification::PUBLIC,
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|e| ComponentError::InvalidConfig(format!("parameter 'classification' is not a classification: {e}")))?,
            };

            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(StaticSystemPromptProvider {
                prompt,
                position,
                classification,
            }));
            Ok(facets)
        },
    );
}
