//! Logging in (issue 10): what a user logs in with, and who they are then.

use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: UserId,
    pub username: String,
    /// Lifetime of a session token in seconds, so that a client knows how
    /// often to renew even if it cannot read the expiry cookie.
    pub token_lifetime_secs: u64,
}
