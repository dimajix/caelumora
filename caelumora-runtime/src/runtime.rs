use crate::component::ComponentRegistry;
use crate::component_impl::{context, history, tool};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::llm::LLMClient;
use crate::llm_impl::ollama::OllamaClient;
use crate::moderator::{BroadcastModerator, Moderator, RandomModerator, RoundRobinModerator};
use crate::recorder::Recorder;
use anyhow::Result;
use caelumora_model::channel::ChannelId;
use caelumora_model::message::ActorId;
use caelumora_model::security::SecurityContext;
use caelumora_model::security::{DummySecurityManager, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StateStore};
use std::sync::Arc;

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    entity_store: Arc<dyn EntityStore>,
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
}

impl RuntimeManager {
    pub fn new(state_store: Arc<dyn StateStore>, entity_store: Arc<dyn EntityStore>) -> Self {
        let mut component_registry = ComponentRegistry::new();
        history::register(&mut component_registry);
        context::register(&mut component_registry);
        tool::register(&mut component_registry);

        let security_manager = Arc::new(DummySecurityManager::new());

        let event_bus = Arc::new(EventBus::new());
        let controller_manager = Arc::new(ControllerManager::new(entity_store.clone(), event_bus.clone()));

        Self {
            security_manager,
            state_store,
            entity_store,
            llm_client: Arc::new(OllamaClient::default()),
            component_registry: Arc::new(component_registry),
            event_bus,
            controller_manager,
        }
    }


    pub fn event_bus(&self) -> Arc<EventBus> {
        self.event_bus.clone()
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.security_manager.clone() }

    pub fn system_security_context(&self, session_id: SessionId, channel_id: ChannelId) -> SecurityContext {
        SecurityContext {
            initiator_id: ActorId::None,
            sender_id: ActorId::None,
            session_id,
            channel_id,
        }
    }

    pub fn system_event_context(&self, session_id: SessionId, channel_id: ChannelId, channel_name: String) -> crate::actor::EventContext {
        crate::actor::EventContext::new(
            session_id,
            channel_id,
            channel_name,
            ActorId::None,
            "System".to_string(),
        )
    }
    
    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.entity_store.clone() }

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
