use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    ChannelScope { session_id:Uuid, channel:String },
    SessionScope { session_id:Uuid },
    UserScope { user_id:Uuid },
    GroupScope { group_id:Uuid },
    #[default]
    GlobalScope
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub scope: Scope,
    pub owner: Uuid,
    pub description: String,
}