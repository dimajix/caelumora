use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::session::SessionId;
use crate::user::UserId;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    ChannelScope { session_id:SessionId, channel:String },
    SessionScope { session_id:SessionId },
    UserScope { user_id:UserId },
    GroupScope { group_id:Uuid },
    #[default]
    GlobalScope
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub scope: Scope,
    pub owner: UserId,
    pub description: String,
}