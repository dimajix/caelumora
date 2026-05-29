use std::fmt::Debug;
use async_trait::async_trait;
use serde_json::Value;
use caelumora_model::tool::{Tool, ToolExecution};


#[async_trait]
pub trait ToolExecutor: Send + Sync + Debug {
    async fn execute(&self, tool: &Tool, input: Value, execution: &mut ToolExecution) -> anyhow::Result<Value>;
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
    async fn execute(&self, _tool: &Tool, _input: Value, _execution: &mut ToolExecution) -> anyhow::Result<Value> {
        todo!()
    }
}