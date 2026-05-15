use crate::driver::ollama::OllamaClient;
use crate::llm::LLMClient;
use crate::moderator::{BroadcastModerator, Moderator};
use crate::recorder::{BlackholeRecorder, Recorder};
use crate::tool::{DummyToolExecutor, ToolExecutor};
use anyhow::Result;
use std::sync::Arc;
use caelumora_model::store::StateStore;


#[derive(Debug)]
pub struct RuntimeManager {
    state_store: Arc<dyn StateStore>,
    tool_executor: Arc<dyn ToolExecutor>,
    llm_client: Arc<dyn LLMClient>,
}

impl RuntimeManager {
    pub fn new(state_store: Arc<dyn StateStore>) -> Self {
        Self {
            state_store,
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

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> {
        Ok(Box::new(BroadcastModerator::new()))
    }

    pub fn recorder(&self, kind: &String) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(BlackholeRecorder::new()))
    }
}
