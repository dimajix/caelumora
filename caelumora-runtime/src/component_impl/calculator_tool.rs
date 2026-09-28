//! The `calculator_tool` component: a tool that evaluates mathematical
//! expressions.

use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolParameters, ToolConfidentiality};
use serde_json::Value;
use std::sync::Arc;

pub const CALCULATOR_TOOL_PROVIDER_KIND: &str = "calculator_tool";

#[derive(Debug)]
pub struct CalculatorToolProvider;

#[async_trait]
impl ToolProvider for CalculatorToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "calculate".to_string(),
            description: "Evaluates a mathematical expression".to_string(),
            parameters: ToolParameters::object(
                vec![("expression", ToolParameters::string("Mathematical expression to evaluate (e.g., '2 + 2', 'sin(pi/2)', 'exp(1)')"))],
                vec!["expression"],
            ),
            requires_approval: false,
            confidentiality: ToolConfidentiality::PUBLIC,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &mut TurnContext) -> ToolResult<Value> {
        if name == "calculate" {
            let expression = params["expression"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidParameter("Missing 'expression' parameter".to_string()))?;

            let result = meval::eval_str(expression)
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to evaluate expression: {}", e)))?;

            serde_json::to_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        CALCULATOR_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CalculatorToolProvider));
            Ok(facets)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    #[tokio::test]
    async fn test_calculator_tool() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

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
            let result = provider.execute("calculate", &params, &mut turn_context).await.unwrap();
            let val = result.as_f64().unwrap();
            assert!((val - expected).abs() < 1e-10, "Expression {} failed: expected {}, got {}", expr, expected, val);
        }
    }

    #[tokio::test]
    async fn test_calculator_tool_invalid_expression() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        let params = json!({ "expression": "invalid" });
        let result = provider.execute("calculate", &params, &mut turn_context).await;
        assert!(result.is_err());
    }
}
