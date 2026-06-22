use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::Tool;
use serde_json::Value;
use std::fmt::Debug;

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;

    /// Executes a specific tool by name.
    async fn execute(&self, name: &str, params: Value, security: &SecurityContext) -> Result<Value>;
}
