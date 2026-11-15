//! The `calculator` component: a tool that evaluates mathematical
//! expressions.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{
    arguments, described, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider,
    ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const CALCULATOR_KIND: &str = "calculator";

pub const CALCULATE_TOOL_NAME: &str = "calculate";

/// The tool `calculate`, which evaluates a mathematical expression.
#[derive(Debug)]
struct CalculateTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for CalculateTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct Expression {
            expression: String,
        }
        let Expression { expression } = arguments(params)?;

        let result = meval::eval_str(&expression).map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to evaluate expression: {}", e))
        })?;

        // Derived from the arguments alone.
        let value =
            serde_json::to_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolOutput::of_turn(value, turn))
    }
}

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let calculate = CalculateTool {
        description: described(
            prompts,
            NAMESPACE,
            CALCULATOR_KIND,
            ToolDescription {
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
            },
            (),
        )?,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, CALCULATOR_KIND, "description", ())?,
        vec![Arc::new(calculate)],
    ))
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(CALCULATOR_KIND).provides::<dyn ToolProvider>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    #[tokio::test]
    async fn test_calculator_tool() {
        let calculate =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), CALCULATE_TOOL_NAME);
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
            let result = calculate.execute(&params, None, &mut turn).await.unwrap();
            let val = result.value.as_f64().unwrap();
            assert!(
                (val - expected).abs() < 1e-10,
                "Expression {} failed: expected {}, got {}",
                expr,
                expected,
                val
            );
        }
    }

    #[tokio::test]
    async fn test_calculator_tool_invalid_expression() {
        let calculate =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), CALCULATE_TOOL_NAME);
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let params = json!({ "expression": "invalid" });
        let result = calculate.execute(&params, None, &mut turn).await;
        assert!(result.is_err());
    }
}
