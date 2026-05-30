use std::collections::{HashMap, HashSet};
use std::fmt::Debug;

use crate::actor::{Actor, ActorEvent, Envelope};
use caelumora_model::message::{ActorId, Message, MessageRole};
use dashmap::DashSet;

pub trait Moderator : Send + Sync + Debug {
    fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)>;
    fn add_actor(&self, actor: &dyn Actor) -> ();
    fn remove_actor(&self, actor: &dyn Actor) -> ();
}

#[derive(Debug)]
pub struct BroadcastModerator {
    actors: DashSet<ActorId>,
}

impl BroadcastModerator {
    pub fn new() -> Self {
        Self {
            actors: DashSet::new(),
        }
    }
}

impl BroadcastModerator {
    fn handle_full_response(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            let sender_ids:HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
            let user_messages = self.replace_role(messages, MessageRole::User);
            let forward_event = event.forward_with_response(user_messages);
            let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
            self.actors.iter()
                .filter(|id| !sender_ids.contains(id))
                .flat_map(|id| vec![(*id, forward_event.clone()), (*id, request_response_event.clone())])
                .collect()
        }
        else {
            Vec::new()
        }
    }

    fn replace_role(&self, messages:&[Message], role:MessageRole) -> Vec<Message> {
        let mut result = messages.to_vec();
        result.iter_mut().for_each(|m| { m.role = role.clone() });
        result
    }
}
impl Moderator for BroadcastModerator {
    fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event)
            }
            _ => {
                Vec::new()
            }
        }
    }

    fn add_actor(&self, actor: &dyn Actor) -> () {
        self.actors.insert(actor.id());
    }

    fn remove_actor(&self, agent: &dyn Actor) -> () {
        self.actors.remove(&agent.id());
    }
}
