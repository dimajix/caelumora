//! The `echo_tool` component: a tool that echoes its input, for testing.

use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{described, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptError};
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
    pub fn new(prompts: &PromptCatalogue) -> Result<Self, PromptError> {
        let tools = vec![described(prompts, ECHO_TOOL_KIND, Tool {
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
        Ok(Self { description: tool_text(prompts, ECHO_TOOL_KIND, "description", ())?, tools })
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
    registry.register(
        ComponentKind::new(ECHO_TOOL_KIND).provides::<dyn ToolProvider>().uses::<PromptCatalogue>(),
        |_descriptor, context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider::new(context.service::<PromptCatalogue>()?)?));
            Ok(facets)
        },
    );
}
