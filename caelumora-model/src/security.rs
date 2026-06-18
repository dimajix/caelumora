use async_trait::async_trait;
use std::fmt::Debug;
use serde::{Deserialize, Serialize};
use crate::channel::ChannelId;
use crate::message::ActorId;
use crate::session::SessionId;

use crate::agent::{Agent, AgentId, Personality, PersonalityId};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::session::Session;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ActionType<'a> {
    GetPersonality(PersonalityId),
    FindPersonality,
    ListPersonalities,
    #[serde(skip)]
    PutPersonality(&'a Personality),
    DeletePersonality(PersonalityId),
    FindSessionTemplate,
    ListSessionTemplates,
    #[serde(skip)]
    PutSessionTemplate(&'a SessionTemplate),
    DeleteSessionTemplate(SessionTemplateId),
    #[serde(skip)]
    CreateAgent(&'a Agent),
    GetAgent(AgentId),
    FindAgent,
    UpdateAgent(AgentId),
    #[serde(skip)]
    CreateSession(&'a Session),
    GetSession(SessionId),
    UpdateSession(SessionId),
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
