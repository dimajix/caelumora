use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::ToolProvider;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::actor::ActorState;
use caelumora_model::tool::{Tool, ToolParameters};
use log::info;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;

pub const ECHO_TOOL_PROVIDER_KIND: &str = "echo_tool";
pub const CALCULATOR_TOOL_PROVIDER_KIND: &str = "calculator_tool";
pub const STOP_TOOL_PROVIDER_KIND: &str = "stop_tool";

#[derive(Debug)]
pub struct StopToolProvider;

#[async_trait]
impl ToolProvider for StopToolProvider {
    fn tools(&self) -> Vec<Tool> {
        let properties: Vec<(String, ToolParameters)> = vec![];
        let required: Vec<String> = vec![];
        vec![
            Tool {
                name: "stop".to_string(),
                description: "Signal that you have finished your task and have no further input.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: true,
            },
            Tool {
                name: "bye".to_string(),
                description: "Signal that you want to say good-bye and stop the conversation.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: true,
            },
            Tool {
                name: "silence".to_string(),
                description: "Signal that you do not want to respond to the last message and stay silent instead.".to_string(),
                parameters: ToolParameters::object(properties, required),
                requires_approval: true,
            }
        ]
    }

    async fn execute(&self, name: &str, _params: &Value, context: &TurnContext) -> Result<Value> {
        match name {
            "stop" | "bye" => {
                info!("Stopping agent {} {}", context.agent_id, context.agent_name);
                context.set_processing_state(ActorState::Sleep);
                Ok(serde_json::json!({ "status": "stopped" }))
            },
            "silence" => {
                info!("Silencing agent {} {}", context.agent_id, context.agent_name);
                context.set_processing_state(ActorState::Silence);
                Ok(serde_json::json!({ "status": "silent" }))
            },
            &_ => {
                Err(anyhow!("Unknown tool {}", name))
            }
        }
    }
}

#[derive(Debug)]
pub struct EchoToolProvider;

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "echo".to_string(),
            description: "Echoes the input back".to_string(),
            parameters: ToolParameters::object(
                vec![("message", ToolParameters::string("Input message to be echoed back"))],
                vec!["message"],
            ),
            requires_approval: false,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &TurnContext) -> Result<Value> {
        if name == "echo" {
            Ok(params.clone())
        } else {
            Err(anyhow::anyhow!("Unknown tool: {}", name))
        }
    }
}

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
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &TurnContext) -> Result<Value> {
        if name == "calculate" {
            let expression = params["expression"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing 'expression' parameter"))?;
            
            let result = meval::eval_str(expression)
                .map_err(|e| anyhow::anyhow!("Failed to evaluate expression: {}", e))?;
            
            Ok(serde_json::to_value(result)?)
        } else {
            Err(anyhow::anyhow!("Unknown tool: {}", name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
    use serde_json::json;

    #[tokio::test]
    async fn test_calculator_tool() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

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
            let result = provider.execute("calculate", &params, &turn_context).await.unwrap();
            let val = result.as_f64().unwrap();
            assert!((val - expected).abs() < 1e-10, "Expression {} failed: expected {}, got {}", expr, expected, val);
        }
    }

    #[tokio::test]
    async fn test_calculator_tool_invalid_expression() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let params = json!({ "expression": "invalid" });
        let result = provider.execute("calculate", &params, &turn_context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stop_tool() {
        let provider = StopToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let result = provider.execute("stop", &json!({}), &turn_context).await.unwrap();
        assert_eq!(result["status"], "stopped");

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "stop");
        assert_eq!(tools[1].name, "bye");
        assert_eq!(tools[2].name, "silence");
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ECHO_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider));
            Ok(facets)
        }),
    );
    registry.register(
        CALCULATOR_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CalculatorToolProvider));
            Ok(facets)
        }),
    );
    registry.register(
        STOP_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(StopToolProvider));
            Ok(facets)
        }),
    );
}
