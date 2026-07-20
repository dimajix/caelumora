use crate::component::ComponentRegistry;
use crate::component_impl::{context, history, mcp, tool};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::llm::LLMClient;
use crate::llm_impl::ollama::OllamaClient;
use crate::moderator::Moderator;
use crate::moderator_impl::broadcast::BroadcastModerator;
use crate::moderator_impl::random::RandomModerator;
use crate::moderator_impl::round_robin::RoundRobinModerator;
use crate::recorder::Recorder;
use crate::tool::{DefaultToolExecutor, ToolExecutor, ToolProvider};
use anyhow::Result;
use caelumora_model::component::ComponentConfig;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{DummySecurityManager, SecurityManager};
use caelumora_model::store::{EntityStore, StateStore};
use std::sync::Arc;

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    entity_store: Arc<dyn EntityStore>,
    secret_manager: Arc<SecretManager>,
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
}

#[derive(Debug, Clone)]
pub struct ActorRuntime {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    entity_store: Arc<dyn EntityStore>,
    secret_manager: Arc<SecretManager>,
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
}

impl ActorRuntime {
    pub fn event_bus(&self) -> Arc<EventBus> {
        self.event_bus.clone()
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.security_manager.clone() }

    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.entity_store.clone() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.secret_manager.clone() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.component_registry.clone()
    }

    pub fn controller_manager(&self) -> Arc<ControllerManager> {
        self.controller_manager.clone()
    }

    pub fn new_tool_executor(&self, tool_providers: Vec<Arc<dyn ToolProvider>>) -> Box<dyn ToolExecutor> {
        Box::new(DefaultToolExecutor::new(self.security_manager.clone(), tool_providers))
    }
}

impl RuntimeManager {
    pub fn new(
        state_store: Arc<dyn StateStore>,
        entity_store: Arc<dyn EntityStore>,
        secret_manager: Arc<SecretManager>,
        shared_components: &ComponentConfig,
    ) -> Result<Self> {
        let mut component_registry = ComponentRegistry::new();
        history::register(&mut component_registry);
        context::register(&mut component_registry);
        tool::register(&mut component_registry);
        mcp::register(&mut component_registry);

        component_registry.configure(&shared_components.component_nodes)?;

        let security_manager = Arc::new(DummySecurityManager::new());

        let event_bus = Arc::new(EventBus::new());
        let controller_manager = Arc::new(ControllerManager::new(entity_store.clone(), event_bus.clone()));

        Ok(Self {
            security_manager,
            state_store,
            entity_store,
            secret_manager,
            llm_client: Arc::new(OllamaClient::default()),
            component_registry: Arc::new(component_registry),
            event_bus,
            controller_manager,
        })
    }

    pub fn new_actor_runtime(&self) -> ActorRuntime {
        ActorRuntime {
            security_manager: self.security_manager.clone(),
            state_store: self.state_store.clone(),
            entity_store: self.entity_store.clone(),
            secret_manager: self.secret_manager.clone(),
            llm_client: self.llm_client.clone(),
            component_registry: self.component_registry.clone(),
            event_bus: self.event_bus.clone(),
            controller_manager: self.controller_manager.clone(),
        }
    }

    pub fn event_bus(&self) -> Arc<EventBus> {
        self.event_bus.clone()
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.security_manager.clone() }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.entity_store.clone() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.secret_manager.clone() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.component_registry.clone()
    }

    pub fn controller_manager(&self) -> Arc<ControllerManager> {
        self.controller_manager.clone()
    }

    pub fn new_moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> {
        match kind.as_str() {
            "broadcast" => Ok(Box::new(BroadcastModerator::new())),
            "random" => Ok(Box::new(RandomModerator::new())),
            "round_robin" => Ok(Box::new(RoundRobinModerator::new())),
            _ => Err(anyhow::anyhow!("Unsupported moderator type: {}", kind))
        }
    }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(crate::recorder::BlackholeRecorder::new()))
    }
}
