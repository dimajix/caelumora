//! The live documents of a running session and their one writer each
//! (issue 86): a controller holds the current `Session`, `Channel`, `Agent`
//! or `User`, stores a change first - which is where the permission is
//! checked - and only then takes it over and announces it on the event bus.
//! The documents themselves are the model's; keeping them live, writing
//! them and announcing is the runtime's.
//!
//! Every controller is built the same way - the document, the event
//! publisher, the store - holds a write lock while it changes its document,
//! and takes over changes made elsewhere through `process_global_event`.
//! The user's has no store: users are not changed through it.

mod agent;
mod channel;
mod session;
mod user;

pub use agent::AgentController;
pub use channel::ChannelController;
pub use session::SessionController;
pub use user::UserController;

use crate::event::EventBus;
use caelumora_model::agent::Agent;
use caelumora_model::channel::Channel;
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::EntityStore;
use caelumora_model::user::User;
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

    pub fn new_agent_controller(&self, session_id: SessionId, agent: Agent) -> Arc<AgentController> {
        Arc::new(AgentController::new(agent, session_id, self.event_bus.clone(), self.entity_store.clone()))
    }

    pub fn new_session_controller(&self, session: Session) -> Arc<SessionController> {
        Arc::new(SessionController::new(session, self.event_bus.clone(), self.entity_store.clone()))
    }

    pub fn new_channel_controller(&self, channel: Channel) -> Arc<ChannelController> {
        Arc::new(ChannelController::new(channel, self.event_bus.clone(), self.entity_store.clone()))
    }

    pub fn new_user_controller(&self, user: User) -> Arc<UserController> {
        Arc::new(UserController::new(user))
    }
}
