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


/// What a user says about themselves (issue 176): the introduction an agent
/// is given of them in every channel they take part in, marked there as
/// written by the user themselves. Public, like their name.
///
/// A profile belongs to its user, unlike the `User` record, which belongs
/// to the system (issue 94): the user writes it, and nothing in it grants
/// anything. One per user, stored under the user's id, and owned by that
/// user - derived from `user_id` rather than stored, so that nobody can
/// write a profile of someone else that is theirs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "StoredUserProfile")]
pub struct UserProfile {
    user_id: UserId,
    /// How the user introduces themselves; `None` if they have not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    #[serde(skip)]
    security_attributes: SecurityAttributes,
}

impl UserProfile {
    /// The profile of user `user_id`, owned by them.
    pub fn new(user_id: UserId, introduction: Option<String>) -> Self {
        let security_attributes = SecurityAttributes { owner_id: ActorId::User(user_id), contributors: Vec::new() };
        Self { user_id, introduction, security_attributes }
    }

    /// The user whose profile it is - also its id in the store.
    pub fn user_id(&self) -> UserId {
        self.user_id
    }
}

impl Secured for UserProfile {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

/// A profile as stored: without its owner, which is derived.
#[derive(Deserialize)]
struct StoredUserProfile {
    user_id: UserId,
    #[serde(default)]
    introduction: Option<String>,
}

impl From<StoredUserProfile> for UserProfile {
    fn from(stored: StoredUserProfile) -> Self {
        UserProfile::new(stored.user_id, stored.introduction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_belongs_to_its_user_whatever_is_stored() {
        let user = UserId::new();
        let stored = serde_json::json!({
            "user_id": user,
            "introduction": "I run the budget.",
            "security_attributes": { "owner_id": "System" },
        });
        let profile: UserProfile = serde_json::from_value(stored).unwrap();
        assert_eq!(profile.security_attributes().owner_id, ActorId::User(user));
        assert_eq!(profile.introduction.as_deref(), Some("I run the budget."));
        let written = serde_json::to_value(&profile).unwrap();
        assert!(written.get("security_attributes").is_none(), "the owner is not written: {written}");
    }

    #[tokio::test]
    async fn a_user_writes_their_own_profile_and_no_one_else_s() {
        use crate::security::{OwnershipSecurityManager, SecurityContext};
        use crate::session::SessionId;
        use crate::channel::ChannelId;
        use crate::store::{EntityStore, StoreError};
        use crate::store_impl::memory::MemoryStore;
        use std::sync::Arc;

        let store = MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new()));
        let (alice, bob) = (UserId::new(), UserId::new());
        let as_user = |id| SecurityContext::new(ActorId::User(id), SessionId::default(), ChannelId::default());

        let hers = UserProfile::new(alice, Some("I run the budget.".to_string()));
        assert!(matches!(store.create_user_profile(&as_user(bob), &hers).await, Err(StoreError::PermissionDenied)), "nobody writes a profile of someone else");
        store.create_user_profile(&as_user(alice), &hers).await.expect("her own");
        let changed = UserProfile::new(alice, Some("Bob decides everything.".to_string()));
        assert!(matches!(store.update_user_profile(&as_user(bob), &changed).await, Err(StoreError::NotFound { .. })), "nor changes it");
        assert_eq!(store.get_user_profile(&SecurityContext::system(), alice).await.unwrap().introduction.as_deref(), Some("I run the budget."));
    }
}
