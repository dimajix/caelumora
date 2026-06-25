use crate::event::EventBus;
use caelumora_model::agent::{Agent, AgentController};
use caelumora_model::channel::{Channel, ChannelController};
use caelumora_model::session::{Session, SessionController, SessionId};
use caelumora_model::store::EntityStore;
use caelumora_model::user::{User, UserController};
use std::sync::Arc;

#[derive(Debug)]
pub struct ControllerManager {
    entity_store: Arc<dyn EntityStore>,
    event_bus: Arc<EventBus>,
}

impl ControllerManager {
    pub fn new(entity_store: Arc<dyn EntityStore>, event_bus: Arc<EventBus>) -> Self {
        Self {
            entity_store,
            event_bus,
        }
    }

    pub async fn new_agent_controller(
        &self,
        session_id: SessionId,
        agent: Agent,
    ) -> Arc<AgentController> {
        Arc::new(AgentController::new(
            session_id,
            agent,
            self.event_bus.clone(),
            self.entity_store.clone(),
        ))
    }

    pub async fn new_session_controller(&self, session: Session) -> anyhow::Result<Arc<SessionController>> {
        Ok(Arc::new(
            SessionController::new(session, self.event_bus.clone(), self.entity_store.clone()).await?,
        ))
    }

    pub fn new_channel_controller(
        &self,
        channel: Channel,
    ) -> Arc<ChannelController> {
        Arc::new(ChannelController::new(channel, self.event_bus.clone()))
    }

    pub fn new_user_controller(
        &self,
        user: User,
    ) -> Arc<UserController> {
        Arc::new(UserController::new(user, self.event_bus.clone()))
    }
}
