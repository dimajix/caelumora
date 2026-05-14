use crate::driver::ollama::OllamaClient;
use crate::llm::LLMClient;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::tool::{DummyToolExecutor, ToolExecutor};
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug)]
pub struct RuntimeManager {
    tool_executor: Arc<dyn ToolExecutor>,
    llm_client: Arc<dyn LLMClient>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            tool_executor: Arc::new(DummyToolExecutor::new()),
            llm_client: Arc::new(OllamaClient::default()),
        }
    }

    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        self.tool_executor.clone()
    }
    
    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }

    pub fn moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> {
        todo!()
    }

    pub fn recorder(&self, kind: &String) -> Result<Box<dyn Recorder>> {
        todo!()
    }
}
