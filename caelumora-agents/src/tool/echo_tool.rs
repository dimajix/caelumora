//! The `echo_tool` component: a tool that echoes its input, for testing.

use crate::prompt::{NAMESPACE, texts_of, writing_for_the_model};
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{described, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_model::question::Answers;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const ECHO_TOOL_KIND: &str = "echo_tool";

pub const ECHO_TOOL_NAME: &str = "echo";

#[derive(Debug)]
pub struct EchoToolProvider {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl EchoToolProvider {
    /// The provider, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let tools = vec![described(prompts, NAMESPACE, ECHO_TOOL_KIND, Tool {
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
        }, ())?];
        Ok(Self { description: tool_text(prompts, NAMESPACE, ECHO_TOOL_KIND, "description", ())?, tools })
    }
}

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name == ECHO_TOOL_NAME {
            // The arguments themselves: classified as the turn (issue 168).
            Ok(ToolOutput::classified(params.clone(), turn.situation().label()))
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(ECHO_TOOL_KIND).provides::<dyn ToolProvider>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider::new(&texts)?));
            Ok(facets)
        },
    );
}
