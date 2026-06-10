use crate::llm_impl::ollama::OllamaClient;
use crate::llm::LLMClient;
use crate::moderator::{BroadcastModerator, Moderator, RandomModerator, RoundRobinModerator};
use crate::recorder::{BlackholeRecorder, Recorder};
use crate::tool::{DummyToolExecutor, ToolExecutor};
use crate::component::ComponentRegistry;
use anyhow::Result;
use std::sync::Arc;
use caelumora_model::store::StateStore;
use crate::security::{DummySecurityManager, SecurityManager};

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    tool_executor: Arc<dyn ToolExecutor>,
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
}

impl RuntimeManager {
    pub fn new(state_store: Arc<dyn StateStore>) -> Self {
        Self {
            security_manager: Arc::new(DummySecurityManager::new()),
            state_store,
            tool_executor: Arc::new(DummyToolExecutor::new()),
            llm_client: Arc::new(OllamaClient::default()),
            component_registry: Arc::new(ComponentRegistry::new()),
        }
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.security_manager.clone() }
    
    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        self.tool_executor.clone()
    }
    
    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.component_registry.clone()
    }

    pub fn new_moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> {
        match kind.as_str() {
            "broadcast" => Ok(Box::new(BroadcastModerator::new())),
            "random" => Ok(Box::new(RandomModerator::new())),
            "round_robin" => Ok(Box::new(RoundRobinModerator::new())),
            _ => Err(anyhow::anyhow!("Unsupported moderator type: {}", kind))
        }
    }

    pub fn new_recorder(&self, _kind: &String) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(BlackholeRecorder::new()))
    }
}
