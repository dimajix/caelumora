use crate::event::EventBus;
use caelumora_model::agent::{Agent, AgentController};
use caelumora_model::channel::{Channel, ChannelController, ChannelId};
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
        channel_id: ChannelId,
        agent: Agent,
    ) -> Arc<AgentController> {
        let event_sender = self.event_bus.agent_event_sender(session_id, channel_id, agent.id);
        Arc::new(AgentController::new(
            agent,
            event_sender,
            self.entity_store.clone(),
        ))
    }

    pub async fn new_session_controller(&self, session: Session) -> anyhow::Result<Arc<SessionController>> {
        let event_sender = self.event_bus.session_event_sender(session.id);
        Ok(Arc::new(
            SessionController::new(session, event_sender, self.entity_store.clone()).await?,
        ))
    }

    pub fn new_channel_controller(
        &self,
        session_id: SessionId,
        channel: Channel,
    ) -> Arc<ChannelController> {
        let event_sender = self.event_bus.channel_event_sender(session_id, channel.id);
        Arc::new(ChannelController::new(channel, event_sender))
    }

    pub fn new_user_controller(
        &self,
        user: User,
        channel_name: String,
    ) -> Arc<UserController> {
        let event_sender = self.event_bus.user_event_sender(user.id);
        Arc::new(UserController::new(user, channel_name, event_sender))
    }
}
