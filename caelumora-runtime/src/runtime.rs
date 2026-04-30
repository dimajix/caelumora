use std::sync::Arc;
use caelumora_model::tool::ToolExecutor;
use crate::driver::ollama::OllamaClient;
use crate::llm::LLMClient;

pub struct RuntimeManager {
    tool_executor: Arc<dyn ToolExecutor>,
    llm_client: Arc<dyn LLMClient>,
}

impl RuntimeManager {
    pub fn new(tool_executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            tool_executor,
            llm_client: Arc::new(OllamaClient::default()),
        }
    }

    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        self.tool_executor.clone()
    }
    
    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }
}
