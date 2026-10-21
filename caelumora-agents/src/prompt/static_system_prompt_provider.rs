//! The `static_system_prompt_provider` component: a fixed prompt part, the
//! system prompt by default (parameters `prompt`, `position` and
//! `classification`), under the title `title` if it names one (issue 249),
//! in the form of every part the runtime writes (`agents.titled` of the
//! prompt catalogue, issue 57). It has none unless it names one: the text
//! may carry its own.

use caelumora_runtime::component::{ComponentError, ComponentFacets, ComponentRegistry, ComponentKind};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, NAMESPACE, texts_of, writing_for_the_model};
use caelumora_model::prompt_catalogue::{context, PromptTexts};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use std::sync::Arc;
use serde_json::Value;

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
    async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        Ok(Prompt::classified(self.prompt.clone(), self.classification.clone()))
    }
}

impl StaticSystemPromptProvider {
    /// The provider its configuration `params` describe: `prompt`, under
    /// `title` if named - as `prompts` puts a part under its title -, at
    /// `position`, classified `classification`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if `prompt` is missing or a parameter
    /// is not what it should be; `ComponentError::Prompt` if the form of a
    /// titled part cannot be had.
    fn of_params(params: &Value, prompts: &PromptTexts) -> Result<Self, ComponentError> {
        let prompt = params.get("prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| ComponentError::InvalidConfig("parameter 'prompt' is missing".to_string()))?;
        let prompt = match params.get("title") {
            None => prompt,
            Some(title) => {
                let title = title.as_str().ok_or_else(|| ComponentError::InvalidConfig("parameter 'title' must be a string".to_string()))?;
                // A part that says nothing is no part.
                if prompt.is_empty() { prompt } else { prompts.render(&format!("{NAMESPACE}.titled"), context! { title, text => prompt })? }
            }
        };
        let position = PromptPosition::of_params(params)?;
        let classification = match params.get("classification") {
            None => Classification::PUBLIC,
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| ComponentError::InvalidConfig(format!("parameter 'classification' is not a classification: {e}")))?,
        };
        Ok(Self { prompt, position, classification })
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(STATIC_SYSTEM_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>()),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(StaticSystemPromptProvider::of_params(&descriptor.params, &texts)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use serde_json::json;

    #[test]
    fn the_prompt_is_titled_only_where_a_title_is_named() {
        let prompts = testing::prompts();
        let titled = StaticSystemPromptProvider::of_params(&json!({ "prompt": "You quiz.", "title": "Who you are" }), &prompts).unwrap();
        assert_eq!(titled.prompt, "[Who you are]\nYou quiz.\n");
        let ended = StaticSystemPromptProvider::of_params(&json!({ "prompt": "You quiz.\n", "title": "Who you are" }), &prompts).unwrap();
        assert_eq!(ended.prompt, "[Who you are]\nYou quiz.\n", "one line break at the end");
        let empty = StaticSystemPromptProvider::of_params(&json!({ "prompt": "", "title": "Who you are" }), &prompts).unwrap();
        assert_eq!(empty.prompt, "", "a part that says nothing is no part");
        let plain = StaticSystemPromptProvider::of_params(&json!({ "prompt": "[Frame]\nBe brief." }), &prompts).unwrap();
        assert_eq!(plain.prompt, "[Frame]\nBe brief.", "as written: it may carry a title of its own");
        assert!(matches!(StaticSystemPromptProvider::of_params(&json!({ "prompt": "x", "title": 1 }), &prompts), Err(ComponentError::InvalidConfig(_))));
    }
}
