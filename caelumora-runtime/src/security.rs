use std::fmt::Debug;
use caelumora_model::message::ActorId;
use caelumora_model::session::{ChannelId, SessionId};

#[derive(Debug, Clone)]
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

pub trait SecurityManager : Debug + Sync + Send {

}

#[derive(Debug)]
pub struct DummySecurityManager {

}
impl DummySecurityManager {
    pub fn new() -> Self {
        Self {}
    }
}
impl SecurityManager for DummySecurityManager {
}