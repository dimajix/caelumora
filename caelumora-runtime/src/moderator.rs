use std::collections::HashMap;
use std::fmt::Debug;
use uuid::Uuid;
use anyhow::Result;

use dashmap::DashSet;
use caelumora_model::message::Message;

pub trait Moderator : Send + Sync + Debug {
    fn route(&self, messages: &[Message]) -> HashMap<Uuid,Vec<Message>>;
    fn add_agent(&self, agent_loop: &crate::agent::AgentLoop) -> Result<()>;
    fn remove_agent(&self, agent_loop: &crate::agent::AgentLoop) -> Result<()>;
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

impl Moderator for BroadcastModerator {
    fn route(&self, messages: &[Message]) -> HashMap<Uuid,Vec<Message>> {
        self.agents.iter()
            .filter(|id| !messages.iter().any(|m| m.sender_id == **id))
            .map(|id| (*id, messages.to_vec()))
            .collect()
    }

    fn add_agent(&self, agent_loop: &crate::agent::AgentLoop) -> Result<()> {
        self.agents.insert(agent_loop.agent_id());
        Ok(())
    }

    fn remove_agent(&self, agent_loop: &crate::agent::AgentLoop) -> Result<()> {
        self.agents.remove(&agent_loop.agent_id());
        Ok(())
    }
}
