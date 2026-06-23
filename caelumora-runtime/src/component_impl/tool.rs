use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::ToolProvider;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{Tool, ToolParameters};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

pub const ECHO_TOOL_PROVIDER_KIND: &str = "echo_tool";

#[derive(Debug)]
pub struct EchoToolProvider;

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn tools(&self) -> Vec<Tool> {
        let mut properties = HashMap::new();
        properties.insert(
            "message".to_string(),
            ToolParameters::String {
                description: Some("Input message to be echoed back".to_string()),
            },
        );
        vec![Tool {
            name: "echo".to_string(),
            description: "Echoes the input back".to_string(),
            parameters: ToolParameters::Object {
                properties,
                required: vec!["message".to_string()],
            },
        }]
    }

    async fn execute(&self, name: &str, params: Value, _security: &SecurityContext) -> Result<Value> {
        if name == "echo" {
            Ok(params)
        } else {
            Err(anyhow::anyhow!("Unknown tool: {}", name))
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ECHO_TOOL_PROVIDER_KIND,
        Arc::new(|_descriptor| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider));
            Ok(facets)
        }),
    );
}
