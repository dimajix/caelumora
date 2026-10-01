//! The `echo_tool` component: a tool that echoes its input, for testing.

use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const ECHO_TOOL_KIND: &str = "echo_tool";

pub const ECHO_TOOL_NAME: &str = "echo";

#[derive(Debug)]
pub struct EchoToolProvider;

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: ECHO_TOOL_NAME.to_string(),
            description: "Echoes the input back".to_string(),
            parameters: ToolParameters::object(
                vec![("message", ToolParameters::string("Input message to be echoed back"))],
                vec!["message"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name == ECHO_TOOL_NAME {
            // The arguments themselves: classified as the turn (issue 168).
            Ok(ToolOutput::classified(params.clone(), turn.label()))
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(ECHO_TOOL_KIND).provides::<dyn ToolProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider));
            Ok(facets)
        },
    );
}
