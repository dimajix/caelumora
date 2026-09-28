//! The `stop_tool` component: tools with which an agent ends its part in a
//! conversation - `stop`, `bye` and `silence`.

use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::actor::ActorState;
use caelumora_model::tool::{Tool, ToolParameters, ToolConfidentiality};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

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
                confidentiality: ToolConfidentiality::PUBLIC,
            },
            Tool {
                name: "bye".to_string(),
                description: "Signal that you want to say good-bye and stop the conversation.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: true,
                confidentiality: ToolConfidentiality::PUBLIC,
            },
            Tool {
                name: "silence".to_string(),
                description: "Signal that you do not want to respond to the last message and stay silent instead.".to_string(),
                parameters: ToolParameters::object(properties, required),
                requires_approval: true,
                confidentiality: ToolConfidentiality::PUBLIC,
            }
        ]
    }

    async fn execute(&self, name: &str, _params: &Value, context: &mut TurnContext) -> ToolResult<Value> {
        match name {
            "stop" | "bye" => {
                info!("Stopping agent {} {}", context.agent_id, context.agent_name);
                context.request_state(ActorState::Sleeping);
                Ok(serde_json::json!({ "status": "stopped" }))
            },
            "silence" => {
                info!("Silencing agent {} {}", context.agent_id, context.agent_name);
                context.request_state(ActorState::Stopping);
                Ok(serde_json::json!({ "status": "silent" }))
            },
            &_ => {
                Err(ToolError::UnknownTool(name.to_string()))
            }
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STOP_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(StopToolProvider));
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
    async fn test_stop_tool() {
        let provider = StopToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        let result = provider.execute("stop", &json!({}), &mut turn_context).await.unwrap();
        assert_eq!(result["status"], "stopped");

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "stop");
        assert_eq!(tools[1].name, "bye");
        assert_eq!(tools[2].name, "silence");
    }
}
