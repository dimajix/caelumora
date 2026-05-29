use std::collections::{HashMap, HashSet};
use std::fmt::Debug;

use crate::actor::{Actor, ActorEvent};
use crate::message::MessageEvent;
use caelumora_model::message::{ActorId, Message, MessageRole};
use dashmap::DashSet;

pub trait Moderator : Send + Sync + Debug {
    fn route(&self, event: &ActorEvent) -> HashMap<ActorId,ActorEvent>;
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
    fn handle_end_of_turn(&self, event: &ActorEvent) -> HashMap<ActorId,ActorEvent> {
        if let MessageEvent::EndOfTurn(messages) = &event.event {
            let sender_ids:HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
            let user_messages = replace_role(messages, MessageRole::User);
            let forward_event = event.forward().with_event(MessageEvent::EndOfTurn(user_messages));
            self.actors.iter()
                .filter(|id| !sender_ids.contains(id))
                .map(|id| (*id, forward_event.clone()))
                .collect()
        }
        else {
            HashMap::new()
        }
    }
}
impl Moderator for BroadcastModerator {
    fn route(&self, event: &ActorEvent) -> HashMap<ActorId,ActorEvent> {
        match event.event {
            MessageEvent::EndOfTurn(_) => {
                self.handle_end_of_turn(event)
            }
            _ => {
                HashMap::new()
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

fn replace_role(messages:&[Message], role:MessageRole) -> Vec<Message> {
    let mut result = messages.to_vec();
    result.iter_mut().for_each(|m| { m.role = role.clone() });
    result
}