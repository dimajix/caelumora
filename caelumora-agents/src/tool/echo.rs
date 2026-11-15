//! The `echo` component: a tool that echoes its input, for testing.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{described, tool_text, FixedTools, Tool, ToolOutput, ToolProvider, ToolResult};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const ECHO_KIND: &str = "echo";

pub const ECHO_TOOL_NAME: &str = "echo";

/// The tool `echo`, which answers with its arguments.
#[derive(Debug)]
struct EchoTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for EchoTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        Ok(ToolOutput::of_turn(params.clone(), turn))
    }
}

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let echo = EchoTool {
        description: described(
            prompts,
            NAMESPACE,
            ECHO_KIND,
            ToolDescription {
                name: ECHO_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    vec![("message", ToolParameters::string())],
                    vec!["message"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            (),
        )?,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, ECHO_KIND, "description", ())?,
        vec![Arc::new(echo)],
    ))
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(ECHO_KIND).provides::<dyn ToolProvider>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            Ok(facets)
        },
    );
}
