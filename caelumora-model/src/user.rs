use crate::actor::ActorId;
use crate::language::Language;
use crate::security::{SecurityAttributes, Secured};
use serde::{Deserialize, Serialize};

define_id!(UserId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub roles: Vec<String>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::removal::Removal>,
}

/// A user record belongs to the system, not to the user (issue 94):
/// everybody may read it - it is the directory that sessions are made
/// from, and lookups are not gated (issue 25) - but only the system, i.e.
/// the user providers and the initial setup, creates, changes or deletes
/// it. Owning it themselves would let users grant themselves `roles`.
/// Derived rather than stored, so that no record can say otherwise.
static SYSTEM_OWNED: SecurityAttributes = SecurityAttributes { owner_id: ActorId::System, contributors: Vec::new() };

crate::impl_deletable!(User, UserProfile);

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
#[serde(from = "UserProfileFields")]
pub struct UserProfile {
    user_id: UserId,
    /// How the user introduces themselves; `None` if they have not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    /// The language the user would like the conversations of the channels
    /// they create to be held in (issue 56); `None` if they did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_language: Option<Language>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::removal::Removal>,
    #[serde(skip)]
    security_attributes: SecurityAttributes,
}

/// The longest introduction of a user, in characters: it goes into the
/// system prompt of every agent in every channel the user takes part in.
pub const MAX_INTRODUCTION_CHARS: usize = 1000;

/// Why a text is no introduction. The messages are meant for users.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntroductionError {
    #[error("an introduction must not be longer than {MAX_INTRODUCTION_CHARS} characters")]
    TooLong,
    #[error("an introduction must not contain control characters other than line breaks and tabs")]
    ControlCharacter,
}

impl UserProfile {
    /// The profile of user `user_id`, introducing them as `introduction`
    /// says, trimmed - none if nothing is left of it. What a user writes
    /// about themselves is checked so, wherever it comes from.
    ///
    /// # Errors
    /// `IntroductionError` if it is longer than `MAX_INTRODUCTION_CHARS` or
    /// contains a control character that is no whitespace.
    pub fn introducing(user_id: UserId, introduction: Option<&str>) -> Result<Self, IntroductionError> {
        let mut profile = Self::new(user_id, None);
        profile.introduce(introduction)?;
        Ok(profile)
    }

    /// Introduces the user as `introduction` says, trimmed - as nobody if
    /// nothing is left of it - checked as `introducing` checks it.
    ///
    /// # Errors
    /// `IntroductionError`, as `introducing`; the profile is then unchanged.
    pub fn introduce(&mut self, introduction: Option<&str>) -> Result<(), IntroductionError> {
        let introduction = introduction.map(str::trim).filter(|text| !text.is_empty());
        if let Some(text) = introduction {
            if text.chars().count() > MAX_INTRODUCTION_CHARS {
                return Err(IntroductionError::TooLong);
            }
            if text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
                return Err(IntroductionError::ControlCharacter);
            }
        }
        self.introduction = introduction.map(str::to_string);
        Ok(())
    }

    /// The profile of user `user_id`, owned by them.
    pub fn new(user_id: UserId, introduction: Option<String>) -> Self {
        let security_attributes = SecurityAttributes { owner_id: ActorId::User(user_id), contributors: Vec::new() };
        Self { user_id, introduction, preferred_language: None, deleted: None, security_attributes }
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

/// A profile as it is written down: without its owner, which is derived
/// from `user_id` when it is read.
#[derive(Deserialize)]
struct UserProfileFields {
    user_id: UserId,
    #[serde(default)]
    introduction: Option<String>,
    #[serde(default)]
    preferred_language: Option<Language>,
    #[serde(default)]
    deleted: Option<crate::removal::Removal>,
}

impl From<UserProfileFields> for UserProfile {
    fn from(fields: UserProfileFields) -> Self {
        UserProfile { preferred_language: fields.preferred_language, deleted: fields.deleted, ..UserProfile::new(fields.user_id, fields.introduction) }
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
        assert_eq!(profile.preferred_language, None, "a profile stored before issue 56 prefers none");
        let written = serde_json::to_value(&profile).unwrap();
        assert!(written.get("security_attributes").is_none(), "the owner is not written: {written}");
    }

    #[test]
    fn a_preferred_language_is_kept_with_the_profile() {
        let profile = UserProfile { preferred_language: Some(Language::parse("de").unwrap()), ..UserProfile::new(UserId::new(), None) };
        let written = serde_json::to_value(&profile).unwrap();
        assert_eq!(written["preferred_language"], "de");
        let read: UserProfile = serde_json::from_value(written).unwrap();
        assert_eq!(read.preferred_language, profile.preferred_language);
    }

    #[test]
    fn an_introduction_is_trimmed_and_checked() {
        let user = UserId::new();
        let profile = UserProfile::introducing(user, Some("  I run the budget.\nAsk me.  ")).unwrap();
        assert_eq!(profile.introduction.as_deref(), Some("I run the budget.\nAsk me."));
        assert_eq!(UserProfile::introducing(user, Some(" \n ")).unwrap().introduction, None, "nothing in it is none");
        assert_eq!(UserProfile::introducing(user, Some(&"x".repeat(MAX_INTRODUCTION_CHARS + 1))).unwrap_err(), IntroductionError::TooLong);
        assert_eq!(UserProfile::introducing(user, Some("a\u{1b}[2Jb")).unwrap_err(), IntroductionError::ControlCharacter);

        let mut introduced = profile.clone();
        assert!(introduced.introduce(Some("a\u{1b}[2Jb")).is_err());
        assert_eq!(introduced.introduction, profile.introduction, "unchanged by what is refused");
        introduced.introduce(None).unwrap();
        assert_eq!(introduced.introduction, None);
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
