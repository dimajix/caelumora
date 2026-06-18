use serde::{Deserialize, Serialize};
use crate::channel::ChannelId;
use crate::message::ActorId;
use crate::session::SessionId;

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
