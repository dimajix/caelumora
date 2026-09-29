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
//!
//! A session's users and agents have one controller each for the whole
//! session, `ParticipantControllers` (issue 120), shared by their actors in
//! all its channels.

mod agent;
mod channel;
mod participants;
mod session;
mod user;

pub use agent::AgentController;
pub use channel::ChannelController;
pub use participants::ParticipantControllers;
pub use session::SessionController;
pub use user::UserController;

use crate::event::EventBus;
use caelumora_model::agent::Agent;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use caelumora_model::user::User;
use std::sync::Arc;
use tracing::warn;

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

    /// The controllers of the users and agents among `participants` of
    /// session `session_id`, from the store. Read as the runtime: that they
    /// take part was permitted when they joined. One that no longer exists
    /// is left out, with a warning - the session runs without it.
    ///
    /// # Errors
    /// Whatever else the store reports.
    pub async fn participant_controllers(&self, session_id: SessionId, participants: &[Participant]) -> StoreResult<ParticipantControllers> {
        let system = SecurityContext::system();
        let (mut users, mut agents) = (Vec::new(), Vec::new());
        for participant in participants {
            let found = match participant {
                Participant::User(id) => self.entity_store.get_user(&system, *id).await
                    .map(|user| users.push(self.new_user_controller(user))),
                Participant::Agent(id) => self.entity_store.get_agent(&system, *id).await
                    .map(|agent| agents.push(self.new_agent_controller(session_id, agent))),
                Participant::Approver(_) => Ok(()),
            };
            match found {
                Ok(()) => {}
                Err(StoreError::NotFound { .. }) => warn!("{} of session {} no longer exists and is left out", participant.actor_id(), session_id),
                Err(error) => return Err(error),
            }
        }
        Ok(ParticipantControllers::new(users, agents))
    }
}
