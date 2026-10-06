//! The `stop_tool` component: tools with which an agent ends its part in a
//! conversation - `stop`, `bye` and `silence`.

use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::actor::ActorState;
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

pub const STOP_TOOL_KIND: &str = "stop_tool";

pub const STOP_TOOL_NAME: &str = "stop";
pub const BYE_TOOL_NAME: &str = "bye";
pub const SILENCE_TOOL_NAME: &str = "silence";

#[derive(Debug)]
pub struct StopToolProvider;

#[async_trait]
impl ToolProvider for StopToolProvider {
    fn description(&self) -> String {
        "End your part in the conversation.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        let properties: Vec<(String, ToolParameters)> = vec![];
        let required: Vec<String> = vec![];
        vec![
            Tool {
                name: STOP_TOOL_NAME.to_string(),
                description: "Signal that you have finished your task and have no further input.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
            },
            Tool {
                name: BYE_TOOL_NAME.to_string(),
                description: "Signal that you want to say good-bye and stop the conversation.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
            },
            Tool {
                name: SILENCE_TOOL_NAME.to_string(),
                description: "Signal that you do not want to respond to the last message and stay silent instead.".to_string(),
                parameters: ToolParameters::object(properties, required),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
            }
        ]
    }

    async fn execute(&self, name: &str, _params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        // What it answers tells nothing: classified as the turn (issue 168).
        match name {
            STOP_TOOL_NAME | BYE_TOOL_NAME => {
                info!("Stopping agent {} {}", turn.agent_id(), turn.agent_name());
                turn.request_state(ActorState::Sleeping);
                Ok(ToolOutput::classified(serde_json::json!({ "status": "stopped" }), turn.label()))
            },
            SILENCE_TOOL_NAME => {
                info!("Silencing agent {} {}", turn.agent_id(), turn.agent_name());
                turn.request_state(ActorState::Idle);
                Ok(ToolOutput::classified(serde_json::json!({ "status": "silent" }), turn.label()))
            },
            &_ => {
                Err(ToolError::UnknownTool(name.to_string()))
            }
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(STOP_TOOL_KIND).provides::<dyn ToolProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(StopToolProvider));
            Ok(facets)
        },
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
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        let result = provider.execute(STOP_TOOL_NAME, &json!({}), &mut turn).await.unwrap();
        assert_eq!(result.value["status"], "stopped");
        assert_eq!(turn.requested_state(), Some(ActorState::Sleeping), "the turn ends, the agent sleeps");
        provider.execute(SILENCE_TOOL_NAME, &json!({}), &mut turn).await.unwrap();
        assert_eq!(turn.requested_state(), Some(ActorState::Idle), "the turn ends, the agent waits");

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, STOP_TOOL_NAME);
        assert_eq!(tools[1].name, BYE_TOOL_NAME);
        assert_eq!(tools[2].name, SILENCE_TOOL_NAME);
    }
}
