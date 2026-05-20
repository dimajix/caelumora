use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Debug;
use uuid::Uuid;

use crate::actor::ActorEvent;
use crate::message::MessageEvent;
use caelumora_model::agent::Agent;
use caelumora_model::message::{Message, MessageRole};
use dashmap::DashSet;

pub trait Moderator : Send + Sync + Debug {
    fn route(&self, event: &ActorEvent) -> HashMap<Uuid,ActorEvent>;
    fn add_agent(&self, agent: &Agent) -> Result<()>;
    fn remove_agent(&self, agent: &Agent) -> Result<()>;
}

#[derive(Debug)]
pub struct BroadcastModerator {
    agents: DashSet<Uuid>,
}

impl BroadcastModerator {
    pub fn new() -> Self {
        Self {
            agents: DashSet::new(),
        }
    }
}

impl BroadcastModerator {
    fn handle_end_of_turn(&self, event: &ActorEvent) -> HashMap<Uuid,ActorEvent> {
        if let MessageEvent::EndOfTurn(messages) = &event.event {
            let user_messages = replace_role(messages, MessageRole::User);
            self.agents.iter()
                .filter(|id| !user_messages.iter().any(|m| m.sender_id == **id))
                .map(|id| (*id, event.next(*id, MessageEvent::EndOfTurn(user_messages.clone()))))
                .collect()
        }
        else {
            HashMap::new()
        }
    }
}
impl Moderator for BroadcastModerator {
    fn route(&self, event: &ActorEvent) -> HashMap<Uuid,ActorEvent> {
        match event.event {
            MessageEvent::EndOfTurn(_) => {
                self.handle_end_of_turn(event)
            }
            _ => {
                HashMap::new()
            }
        }
    }

    fn add_agent(&self, agent: &Agent) -> Result<()> {
        self.agents.insert(agent.id);
        Ok(())
    }

    fn remove_agent(&self, agent: &Agent) -> Result<()> {
        self.agents.remove(&agent.id);
        Ok(())
    }
}

fn replace_role(messages:&[Message], role:MessageRole) -> Vec<Message> {
    let mut result = messages.to_vec();
    result.iter_mut().for_each(|m| { m.role = role.clone() });
    result
}