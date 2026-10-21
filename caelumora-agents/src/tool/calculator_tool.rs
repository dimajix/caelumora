//! The `calculator_tool` component: a tool that evaluates mathematical
//! expressions.

use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{described, arguments, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptError};
use caelumora_model::question::Answers;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const CALCULATOR_TOOL_KIND: &str = "calculator_tool";

pub const CALCULATE_TOOL_NAME: &str = "calculate";

#[derive(Debug)]
pub struct CalculatorToolProvider {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl CalculatorToolProvider {
    /// The provider, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: &PromptCatalogue) -> Result<Self, PromptError> {
        let tools = vec![described(prompts, CALCULATOR_TOOL_KIND, Tool {
            name: CALCULATE_TOOL_NAME.to_string(),
            description: String::new(),
            parameters: ToolParameters::object(
                vec![("expression", ToolParameters::string())],
                vec!["expression"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: Default::default(),
        }, ())?];
        Ok(Self { description: tool_text(prompts, CALCULATOR_TOOL_KIND, "description", ())?, tools })
    }
}

#[async_trait]
impl ToolProvider for CalculatorToolProvider {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name == CALCULATE_TOOL_NAME {
            #[derive(Deserialize)]
            struct Calculate { expression: String }
            let Calculate { expression } = arguments(params)?;

            let result = meval::eval_str(&expression)
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to evaluate expression: {}", e)))?;

            // Derived from the arguments alone: classified as the turn (issue 168).
            let value = serde_json::to_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolOutput::classified(value, turn.situation().label()))
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CALCULATOR_TOOL_KIND).provides::<dyn ToolProvider>().uses::<PromptCatalogue>(),
        |_descriptor, context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CalculatorToolProvider::new(context.service::<PromptCatalogue>()?)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    #[tokio::test]
    async fn test_calculator_tool() {
        let provider = CalculatorToolProvider::new(&testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let cases = vec![
            ("2 + 2", 4.0),
            ("2 * (3 + 4)", 14.0),
            ("sin(0)", 0.0),
            ("cos(0)", 1.0),
            ("exp(0)", 1.0),
            ("10 / 2", 5.0),
        ];

        for (expr, expected) in cases {
            let params = json!({ "expression": expr });
            let result = provider.execute(CALCULATE_TOOL_NAME, &params, None, &mut turn).await.unwrap();
            let val = result.value.as_f64().unwrap();
            assert!((val - expected).abs() < 1e-10, "Expression {} failed: expected {}, got {}", expr, expected, val);
        }
    }

    #[tokio::test]
    async fn test_calculator_tool_invalid_expression() {
        let provider = CalculatorToolProvider::new(&testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let params = json!({ "expression": "invalid" });
        let result = provider.execute(CALCULATE_TOOL_NAME, &params, None, &mut turn).await;
        assert!(result.is_err());
    }
}
