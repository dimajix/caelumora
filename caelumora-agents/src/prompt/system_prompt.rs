//! The `system_prompt` component: a fixed prompt part, the
//! system prompt by default (parameters `prompt`, `position` and
//! `classification`), under the title `title` if it names one (issue 249),
//! in the form of every part the agents write (`agents.titled` of the
//! prompt catalogue, issue 57). It has none unless it names one: the text
//! may carry its own - but given into the transcript (`position: given`)
//! it names one, since a title in brackets is what marks what is given to
//! read (issue 539).

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, Prompt, PromptPosition, PromptProvider,
    NAMESPACE,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{context, PromptTexts};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const SYSTEM_PROMPT_KIND: &str = "system_prompt";

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemPromptParams {
    prompt: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    position: PromptPosition,
    #[serde(default = "default_classification")]
    classification: Classification,
}

impl Params for SystemPromptParams {}

fn default_classification() -> Classification {
    Classification::PUBLIC
}

#[derive(Debug)]
pub struct SystemPromptComponent {
    pub prompt: String,
    pub position: PromptPosition,
    /// How the prompt is classified (issue 168): public unless configured
    /// otherwise - who writes something confidential into a personality's
    /// prompt classifies it.
    pub classification: Classification,
}

#[async_trait]
impl PromptProvider for SystemPromptComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }
    async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        Ok(Prompt::classified(self.prompt.clone(), self.classification.clone()))
    }
}

impl SystemPromptComponent {
    /// The provider its configuration `params` describe: `prompt`, under
    /// `title` if named - as `prompts` puts a part under its title -, at
    /// `position`, classified `classification`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if `prompt` is missing, a parameter
    /// is not what it should be, or it is given into the transcript without a
    /// `title`; `ComponentError::Prompt` if the form of a titled part cannot
    /// be had.
    fn of_params(params: &Value, prompts: &PromptTexts) -> Result<Self, ComponentError> {
        let SystemPromptParams { prompt, title, position, classification } =
            SystemPromptParams::read(params)?;
        if position == PromptPosition::Given && title.is_none() {
            return Err(ComponentError::InvalidConfig(
                "`title` is required where `position` is `given`: its title marks what is given"
                    .to_string(),
            ));
        }
        let prompt = match title {
            // A part that says nothing is no part.
            Some(title) if !prompt.is_empty() => prompts
                .render(&format!("{NAMESPACE}.titled"), context! { title, text => prompt })?,
            _ => prompt,
        };
        Ok(Self { prompt, position, classification })
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(SYSTEM_PROMPT_KIND)
                .params::<SystemPromptParams>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(SystemPromptComponent::of_params(
                &descriptor.params,
                &texts,
            )?));
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
        let titled = SystemPromptComponent::of_params(
            &json!({ "prompt": "You quiz.", "title": "Who you are" }),
            &prompts,
        )
        .unwrap();
        assert_eq!(titled.prompt, "[Who you are]\nYou quiz.\n");
        let ended = SystemPromptComponent::of_params(
            &json!({ "prompt": "You quiz.\n", "title": "Who you are" }),
            &prompts,
        )
        .unwrap();
        assert_eq!(ended.prompt, "[Who you are]\nYou quiz.\n", "one line break at the end");
        let empty = SystemPromptComponent::of_params(
            &json!({ "prompt": "", "title": "Who you are" }),
            &prompts,
        )
        .unwrap();
        assert_eq!(empty.prompt, "", "a part that says nothing is no part");
        let plain =
            SystemPromptComponent::of_params(&json!({ "prompt": "[Frame]\nBe brief." }), &prompts)
                .unwrap();
        assert_eq!(
            plain.prompt, "[Frame]\nBe brief.",
            "as written: it may carry a title of its own"
        );
        assert!(matches!(
            SystemPromptComponent::of_params(&json!({ "prompt": "x", "title": 1 }), &prompts),
            Err(ComponentError::InvalidConfig(_))
        ));
        let given = |params: Value| SystemPromptComponent::of_params(&params, &prompts);
        assert!(
            matches!(
                given(json!({ "prompt": "[Rules]\nBe brief.", "position": "given" })),
                Err(ComponentError::InvalidConfig(_))
            ),
            "given into the transcript, it names its title (issue 539)"
        );
        let titled = given(json!({ "prompt": "Be brief.", "title": "Rules", "position": "given" }));
        assert_eq!(titled.unwrap().prompt, "[Rules]\nBe brief.\n");
    }
}
