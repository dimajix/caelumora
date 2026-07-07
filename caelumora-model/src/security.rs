use crate::actor::ActorId;
use crate::agent::{Agent, AgentId, Personality, PersonalityId};
use crate::channel::ChannelId;
use crate::message::ToolCall;
use crate::session::Session;
use crate::session::SessionId;
use crate::template::{SessionTemplate, SessionTemplateId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

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
    UpdateAgent(&'a Agent),
    CreateSession(&'a Session),
    GetSession(SessionId),
    UpdateSession(&'a Session),
    ListSessions,
    CallTool(&'a ToolCall),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub caller_id: ActorId,
    pub delegation_ids: Vec<ActorId>,
    pub session_id: SessionId,
    pub channel_id: ChannelId
}

impl SecurityContext {
    pub fn new(initiator: ActorId, session_id: SessionId, channel_id: ChannelId) -> Self {
        Self {
            initiator_id: initiator,
            caller_id: initiator,
            delegation_ids: vec![initiator],
            session_id,
            channel_id
        }
    }
    pub fn next(&self, caller_id: ActorId) -> Self {
        let mut delegation_ids = self.delegation_ids.clone();
        delegation_ids.push(caller_id);

        Self {
            initiator_id: self.initiator_id,
            caller_id,
            delegation_ids,
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
