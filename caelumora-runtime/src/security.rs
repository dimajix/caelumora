use std::fmt::Debug;
use uuid::Uuid;
use caelumora_model::message::ActorId;

#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub sender_id: ActorId,
}

impl SecurityContext {
    pub fn next(&self, sender_id: ActorId) -> Self {
        Self {
            initiator_id: self.initiator_id,
            sender_id,
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