use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolExecution};
use serde_json::Value;
use std::fmt::Debug;

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;

    /// Executes a specific tool by name.
    async fn execute(&self, name: &str, params: Value, execution: &mut ToolExecution) -> Result<Value>;
}

#[async_trait]
pub trait ToolExecutor: Send + Sync + Debug {
    async fn execute(&self, tool: &Tool, input: Value, execution: &mut ToolExecution) -> Result<Value>;
}

#[derive(Debug)]
pub struct DummyToolExecutor {
}

impl DummyToolExecutor {
    pub fn new() -> Self {
        DummyToolExecutor {}
    }
}

#[async_trait]
impl ToolExecutor for DummyToolExecutor {
    async fn execute(&self, _tool: &Tool, _input: Value, _execution: &mut ToolExecution) -> Result<Value> {
        todo!()
    }
}