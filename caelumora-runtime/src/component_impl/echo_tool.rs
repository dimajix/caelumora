//! The `echo_tool` component: a tool that echoes its input, for testing.

use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const ECHO_TOOL_PROVIDER_KIND: &str = "echo_tool";

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
            visibility: ToolVisibility::Public,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &mut TurnContext) -> ToolResult<Value> {
        if name == "echo" {
            Ok(params.clone())
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
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
}
