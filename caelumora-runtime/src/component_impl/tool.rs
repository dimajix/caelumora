use crate::tool::ToolProvider;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolExecution};
use serde_json::Value;
use std::fmt::Debug;

#[derive(Debug)]
pub struct EchoToolProvider;

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "echo".to_string(),
            description: "Echoes the input back".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                }
            }),
        }]
    }

    async fn execute(&self, name: &str, params: Value, _execution: &mut ToolExecution) -> Result<Value> {
        if name == "echo" {
            Ok(params)
        } else {
            Err(anyhow::anyhow!("Unknown tool: {}", name))
        }
    }
}
