use serde::{Deserialize, Serialize};

define_id!(UserId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserUpdateEvent {
    NameChanged(String),
}

