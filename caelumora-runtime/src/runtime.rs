use crate::llm_impl::ollama::OllamaClient;
use crate::llm::LLMClient;
use crate::moderator::{BroadcastModerator, Moderator, RandomModerator, RoundRobinModerator};
use crate::recorder::{Recorder};
use crate::tool::{DummyToolExecutor, ToolExecutor};
use crate::component::ComponentRegistry;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use caelumora_model::store::{StateStore, TemplateStore};
use crate::component_impl::{context, history};
use crate::security::{DummySecurityManager, SecurityManager, SecurityContext};
use serde::{Deserialize, Serialize};
use caelumora_model::session::{SessionId, SessionUpdateEvent};
use caelumora_model::channel::{ChannelId, ChannelUpdateEvent};
use caelumora_model::agent::{AgentId, AgentUpdateEvent};
use caelumora_model::message::ActorId;
use caelumora_model::user::{UserId, UserUpdateEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionUpdated(SessionId, SessionUpdateEvent),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    AgentUpdated(AgentId, AgentUpdateEvent),
    UserUpdated(UserId, UserUpdateEvent),
}

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    template_store: Arc<dyn TemplateStore>,
    tool_executor: Arc<dyn ToolExecutor>,
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
    global_event_tx: broadcast::Sender<GlobalEvent>,
}

impl RuntimeManager {
    pub fn new(state_store: Arc<dyn StateStore>, template_store: Arc<dyn TemplateStore>) -> Self {
        let mut component_registry = ComponentRegistry::new();
        history::register(&mut component_registry);
        context::register(&mut component_registry);

        let (global_event_tx, _) = broadcast::channel(100);

        Self {
            security_manager: Arc::new(DummySecurityManager::new()),
            state_store,
            template_store,
            tool_executor: Arc::new(DummyToolExecutor::new()),
            llm_client: Arc::new(OllamaClient::default()),
            component_registry: Arc::new(component_registry),
            global_event_tx,
        }
    }

    pub fn notify_global(&self, event: GlobalEvent) {
        let _ = self.global_event_tx.send(event);
    }

    pub fn subscribe_global(&self) -> broadcast::Receiver<GlobalEvent> {
        self.global_event_tx.subscribe()
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
    
    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        self.tool_executor.clone()
    }
    
    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.llm_client.clone()
    }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn template_store(&self) -> Arc<dyn TemplateStore> { self.template_store.clone() }

    pub fn session_event_sender(self: &Arc<Self>, session_id: SessionId) -> Arc<dyn caelumora_model::event::EventSender<caelumora_model::session::SessionUpdateEvent>> {
        let rm = self.clone();
        Arc::new(caelumora_model::event::FnEventSender::new(move |event| {
            rm.notify_global(GlobalEvent::SessionUpdated(session_id, event));
        }))
    }

    pub fn channel_event_sender(self: &Arc<Self>, session_id: SessionId, channel_id: ChannelId) -> Arc<dyn caelumora_model::event::EventSender<ChannelUpdateEvent>> {
        let rm = self.clone();
        Arc::new(caelumora_model::event::FnEventSender::new(move |event| {
            rm.notify_global(GlobalEvent::ChannelUpdated(session_id, channel_id, event));
        }))
    }

    pub fn agent_event_sender(self: &Arc<Self>, agent_id: AgentId) -> Arc<dyn caelumora_model::event::EventSender<caelumora_model::agent::AgentUpdateEvent>> {
        let rm = self.clone();
        Arc::new(caelumora_model::event::FnEventSender::new(move |event| {
            rm.notify_global(GlobalEvent::AgentUpdated(agent_id, event));
        }))
    }

    pub fn user_event_sender(self: &Arc<Self>, user_id: caelumora_model::user::UserId) -> Arc<dyn caelumora_model::event::EventSender<caelumora_model::user::UserUpdateEvent>> {
        let rm = self.clone();
        Arc::new(caelumora_model::event::FnEventSender::new(move |event| {
            rm.notify_global(GlobalEvent::UserUpdated(user_id, event));
        }))
    }

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

    pub fn new_recorder(&self, state: Arc<Mutex<caelumora_model::channel::ChannelState>>) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(crate::recorder::ChannelHistoryRecorder::new(state)))
    }
}
