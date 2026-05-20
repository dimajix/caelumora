use std::fmt::Debug;
use uuid::Uuid;


#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub initiator_id: Uuid,
    pub sender_id: Uuid,
}

impl SecurityContext {
    pub fn next(&self, sender_id: Uuid) -> Self {
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