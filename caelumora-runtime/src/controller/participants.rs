use super::{AgentController, UserController};
use crate::event::SubscriptionTopic;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::Participant;
use caelumora_model::event::GlobalEvent;
use caelumora_model::user::UserId;
use std::collections::HashMap;
use std::sync::Arc;

/// The live users and agents of one session (issue 120): one controller
/// each, shared by their actors in all of the session's channels, so that a
/// session has one writer per agent and one current view of every user.
/// Built from the store when the session starts; the session keeps them
/// current (`process_global_event`), not the actors.
#[derive(Debug, Default)]
pub struct ParticipantControllers {
    users: HashMap<UserId, Arc<UserController>>,
    agents: HashMap<AgentId, Arc<AgentController>>,
}

impl ParticipantControllers {
    pub(super) fn new(users: Vec<Arc<UserController>>, agents: Vec<Arc<AgentController>>) -> Self {
        Self {
            users: users.into_iter().map(|u| (u.id(), u)).collect(),
            agents: agents.into_iter().map(|a| (a.id(), a)).collect(),
        }
    }

    /// The controller of user `id`, `None` if the user is no participant or
    /// no longer exists.
    pub fn user(&self, id: UserId) -> Option<Arc<UserController>> {
        self.users.get(&id).cloned()
    }

    /// The controller of agent `id`, `None` if the agent is no participant
    /// or no longer exists.
    pub fn agent(&self, id: AgentId) -> Option<Arc<AgentController>> {
        self.agents.get(&id).cloned()
    }

    /// Whether `participant` has a controller here, i.e. whether an actor
    /// can be built for it. An approver has no record and always has.
    pub fn has(&self, participant: &Participant) -> bool {
        match participant {
            Participant::User(id) => self.users.contains_key(id),
            Participant::Agent(id) => self.agents.contains_key(id),
            Participant::Approver(_) => true,
        }
    }

    /// The topics whose events concern these participants: a rename is
    /// announced about the user or agent, whichever session made it.
    pub fn topics(&self) -> Vec<SubscriptionTopic> {
        self.users.keys().map(|id| SubscriptionTopic::User(*id))
            .chain(self.agents.keys().map(|id| SubscriptionTopic::Agent(*id)))
            .collect()
    }

    /// Takes over a change to one of the participants, made and stored
    /// elsewhere, once it is announced.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::UserUpdated(id, _) => if let Some(user) = self.users.get(id) {
                user.process_global_event(event);
            },
            GlobalEvent::AgentUpdated(_, id, _) => if let Some(agent) = self.agents.get(id) {
                agent.process_global_event(event);
            },
            _ => {}
        }
    }
}
