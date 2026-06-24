use crate::channel::ChannelId;
use crate::message::{ActorId, ToolCall};
use crate::session::SessionId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use crate::agent::{Agent, AgentId, Personality, PersonalityId};
use crate::session::Session;
use crate::template::{SessionTemplate, SessionTemplateId};

#[derive(Debug, Clone, Copy)]
pub enum ActionType<'a> {
    GetPersonality(PersonalityId),
    FindPersonality,
    ListPersonalities,
    PutPersonality(&'a Personality),
    DeletePersonality(PersonalityId),
    FindSessionTemplate,
    ListSessionTemplates,
    PutSessionTemplate(&'a SessionTemplate),
    DeleteSessionTemplate(SessionTemplateId),
    CreateAgent(&'a Agent),
    GetAgent(AgentId),
    FindAgent,
    UpdateAgent(AgentId),
    CreateSession(&'a Session),
    GetSession(SessionId),
    UpdateSession(SessionId),
    ListSessions,
    CallTool(&'a ToolCall),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub sender_id: ActorId,
    pub session_id: SessionId,
    pub channel_id: ChannelId
}

impl SecurityContext {
    pub fn next(&self, sender_id: ActorId) -> Self {
        Self {
            initiator_id: self.initiator_id,
            sender_id,
            session_id: self.session_id,
            channel_id: self.channel_id
        }
    }
}

#[async_trait]
pub trait SecurityManager : Debug + Sync + Send {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool;
}

#[derive(Debug)]
pub struct DummySecurityManager {
}

impl DummySecurityManager {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl SecurityManager for DummySecurityManager {
    async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
        true
    }
}
