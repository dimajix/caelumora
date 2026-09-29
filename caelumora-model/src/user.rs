use crate::actor::ActorId;
use crate::security::{SecurityAttributes, Secured};
use serde::{Deserialize, Serialize};

define_id!(UserId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub roles: Vec<String>,
}

/// A user record belongs to the system, not to the user (issue 94):
/// everybody may read it - it is the directory that sessions are made
/// from, and lookups are not gated (issue 25) - but only the system, i.e.
/// the user providers and the initial setup, creates, changes or deletes
/// it. Owning it themselves would let users grant themselves `roles`.
/// Derived rather than stored, so that no record can say otherwise.
static SYSTEM_OWNED: SecurityAttributes = SecurityAttributes { owner_id: ActorId::System, contributors: Vec::new() };

impl Secured for User {
    fn security_attributes(&self) -> &SecurityAttributes {
        &SYSTEM_OWNED
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserUpdateEvent {
    NameChanged(String),
}

