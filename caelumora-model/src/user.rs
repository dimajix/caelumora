use crate::actor::ActorId;
use crate::approval::{ApprovalRule, ApprovalRuleId, RuleError, MAX_PERSONAL_RULES};
use crate::chain::Position;
use crate::message::MessageId;
use crate::removal::Removal;
use crate::role::RoleId;
use crate::scope::{Scope, Scoped};
use crate::security::{Secured, SecurityAttributes};
use crate::store::query::{Field, NoField};
use crate::store::{EntityKind, Journaled, NoEntity, Stored, Updatable};
use crate::value::language::Language;
use crate::value::name::{Name, ScopedName};
use crate::value::text::Introduction;
use crate::value::ValueError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

define_id!(UserId);

/// What a query of users may name (issue 221).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserField {
    /// Their name.
    Username,
}

impl Field for UserField {
    fn path(self) -> &'static [&'static str] {
        match self {
            UserField::Username => &["username"],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    /// An identity, compared as names are (issue 250): two usernames that
    /// differ only in case or accents are one.
    pub username: Name,
    pub email: String,
    /// What the user is (issue 142), by the roles' ids (issue 23): given by
    /// the user provider, never by the user.
    pub roles: Vec<RoleId>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

/// A user record belongs to the system, not to the user (issue 94):
/// everybody may read it - it is the directory that sessions are made
/// from, and lookups are not gated (issue 25) - but only the system, i.e.
/// the user providers and the initial setup, creates, changes or deletes
/// it. Owning it themselves would let users grant themselves `roles`.
/// Derived rather than stored, so that no record can say otherwise.
static SYSTEM_OWNED: SecurityAttributes = SecurityAttributes::systems();

impl Stored for User {
    const KIND: EntityKind = EntityKind::new("user");
    type Id = UserId;
    type Within = NoEntity;
    type Field = UserField;

    fn id(&self) -> UserId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }
}

/// What a change of the user says is made into its values here (issue
/// 248).
impl Updatable for User {
    type Update = UserUpdateEvent;

    fn apply(&mut self, event: &UserUpdateEvent) -> Result<(), ValueError> {
        match event {
            UserUpdateEvent::NameChanged(name) => self.username = Name::new(name)?,
            // As the `UserManager` stored them (issue 306).
            UserUpdateEvent::RolesChanged(roles) => self.roles = roles.clone(),
        }
        Ok(())
    }
}

impl Stored for UserProfile {
    const KIND: EntityKind = EntityKind::new("user_profile");
    type Id = UserId;
    type Within = NoEntity;
    type Field = NoField;

    fn id(&self) -> UserId {
        self.user_id()
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

/// A username is unique everywhere (issue 250).
impl Scoped for User {
    fn scoped_name(&self) -> ScopedName {
        ScopedName { name: self.username.clone(), scope: Scope::GlobalScope }
    }
}

impl Secured for User {
    fn security_attributes(&self) -> &SecurityAttributes {
        &SYSTEM_OWNED
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserUpdateEvent {
    NameChanged(String),
    /// The roles the user's provider gives them now (issue 306), as the
    /// `UserManager` stored them.
    RolesChanged(Vec<RoleId>),
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
    pub introduction: Option<Introduction>,
    /// The language the user would like the conversations of the channels
    /// they create to be held in (issue 56); `None` if they did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_language: Option<Language>,
    /// The user's answers to tool calls given in advance (issue 13): rules
    /// that allow or deny, each within a reach. Read by nobody but the user
    /// and the runtime, unlike the introduction. Changed only through
    /// `add_approval_rule` and `remove_approval_rule`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    approval_rules: Vec<ApprovalRule>,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    #[serde(skip)]
    security_attributes: SecurityAttributes,
}

impl UserProfile {
    /// The profile of user `user_id`, owned by them, introducing them as
    /// `introduction` says.
    pub fn new(user_id: UserId, introduction: Option<Introduction>) -> Self {
        let security_attributes = SecurityAttributes::owned_by(ActorId::User(user_id));
        Self {
            user_id,
            introduction,
            preferred_language: None,
            approval_rules: Vec::new(),
            deleted: None,
            security_attributes,
        }
    }

    /// Introduces the user as `introduction` says - as nobody if it is
    /// missing or empty.
    ///
    /// # Errors
    /// `ValueError` if it does not fit (`Introduction`); the profile is then
    /// unchanged.
    pub fn introduce(&mut self, introduction: Option<&str>) -> Result<(), ValueError> {
        self.introduction = Introduction::optional(introduction)?;
        Ok(())
    }

    /// The user whose profile it is - also its id in the store.
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    /// The user's rules on tool calls (issue 13), in the order they were
    /// added.
    pub fn approval_rules(&self) -> &[ApprovalRule] {
        &self.approval_rules
    }

    /// Adds `rule` to the user's answers given in advance.
    ///
    /// # Errors
    /// `RuleError` if a person may not keep it (`check_personal`) or the
    /// user keeps `MAX_PERSONAL_RULES` already; the profile is then
    /// unchanged.
    pub fn add_approval_rule(&mut self, rule: ApprovalRule) -> Result<(), RuleError> {
        rule.check_personal()?;
        if self.approval_rules.len() >= MAX_PERSONAL_RULES {
            return Err(RuleError::TooMany);
        }
        self.approval_rules.push(rule);
        Ok(())
    }

    /// Takes the rule `id` back; whether there was one.
    pub fn remove_approval_rule(&mut self, id: ApprovalRuleId) -> bool {
        let before = self.approval_rules.len();
        self.approval_rules.retain(|rule| rule.id != id);
        self.approval_rules.len() < before
    }

    /// Drops the rules that expired by `now`: they apply nowhere any more.
    pub fn prune_approval_rules(&mut self, now: DateTime<Utc>) {
        self.approval_rules.retain(|rule| !rule.is_expired(now));
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
    introduction: Option<Introduction>,
    #[serde(default)]
    preferred_language: Option<Language>,
    #[serde(default)]
    approval_rules: Vec<ApprovalRule>,
    #[serde(default)]
    deleted: Option<Removal>,
}

impl From<UserProfileFields> for UserProfile {
    fn from(fields: UserProfileFields) -> Self {
        UserProfile {
            preferred_language: fields.preferred_language,
            approval_rules: fields.approval_rules,
            deleted: fields.deleted,
            ..UserProfile::new(fields.user_id, fields.introduction)
        }
    }
}

/// A user in one channel (issue 404): up to where they read it. The user's
/// counterpart of an agent's `AgentChannelState`, kept in the `StateStore`,
/// written by the user's controller alone and read by nobody but the user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserChannelState {
    /// The last message of someone else the user read; `None` while they
    /// read nothing of the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadMark>,
}

/// Up to where a user read a channel: a message and where it stands in the
/// channel's history - the position to count what is new from, the message
/// to show where the new ones begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMark {
    pub position: Position,
    pub message: MessageId,
}

impl UserChannelState {
    /// Moves the mark to `mark` if that lies after it - it only moves
    /// forward, so that two frontends of the user cannot move it back.
    /// Whether it moved.
    pub fn read_up_to(&mut self, mark: ReadMark) -> bool {
        if self.read.is_some_and(|read| read.position >= mark.position) {
            return false;
        }
        self.read = Some(mark);
        true
    }

    /// The position in the channel's history from which on what is said is
    /// new to the user: after the mark, or from the start.
    pub fn unread_from(&self) -> Position {
        self.read.map_or(Position::START, |read| read.position.next())
    }
}

/// A change to a [`UserChannelState`] since it was last stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserChannelStateChange {
    Read(Option<ReadMark>),
}

impl Journaled for UserChannelState {
    type Change = UserChannelStateChange;

    /// The mark is small and changed in place: recorded whole.
    fn changes(&self) -> Vec<Self::Change> {
        vec![UserChannelStateChange::Read(self.read)]
    }

    fn mark_stored(&mut self) {}

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            UserChannelStateChange::Read(read) => self.read = read,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::ConfidentialityRules;
    use std::slice;

    /// Issue 404: two frontends of one user cannot move the mark back.
    #[test]
    fn what_was_read_only_moves_forward() {
        let mut state = UserChannelState::default();
        assert_eq!(state.unread_from().get(), 0, "nothing read: everything is new");
        let first = ReadMark { position: Position::new(4), message: MessageId::new() };
        let later = ReadMark { position: Position::new(9), message: MessageId::new() };
        assert!(state.read_up_to(first));
        assert!(state.read_up_to(later));
        assert!(!state.read_up_to(first), "not back");
        assert!(!state.read_up_to(later), "nor again");
        assert_eq!((state.read, state.unread_from().get()), (Some(later), 10));
        let mut replayed = UserChannelState::default();
        for change in state.changes() {
            replayed.apply(change).unwrap();
        }
        assert_eq!(replayed.read, Some(later), "replayed as stored");
    }

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
        assert_eq!(
            profile.introduction.as_ref().map(Introduction::as_str),
            Some("I run the budget.")
        );
        assert_eq!(
            profile.preferred_language, None,
            "a profile stored before issue 56 prefers none"
        );
        let written = serde_json::to_value(&profile).unwrap();
        assert!(
            written.get("security_attributes").is_none(),
            "the owner is not written: {written}"
        );
    }

    #[test]
    fn a_preferred_language_is_kept_with_the_profile() {
        let profile = UserProfile {
            preferred_language: Some(Language::new("de").unwrap()),
            ..UserProfile::new(UserId::new(), None)
        };
        let written = serde_json::to_value(&profile).unwrap();
        assert_eq!(written["preferred_language"], "de");
        let read: UserProfile = serde_json::from_value(written).unwrap();
        assert_eq!(read.preferred_language, profile.preferred_language);
    }

    #[test]
    fn a_stored_introduction_is_checked_when_read() {
        let stored = serde_json::json!({ "user_id": UserId::new(), "introduction": "a\u{1b}[2Jb" });
        let refused = serde_json::from_value::<UserProfile>(stored).unwrap_err();
        assert!(
            refused.to_string().contains("the introduction must not contain control characters"),
            "{refused}"
        );
    }

    #[test]
    fn a_profile_keeps_the_rules_a_person_may_keep() {
        use crate::approval::{Reach, RuleDecision, ToolMatcher};
        let mut profile = UserProfile::new(UserId::new(), None);
        let shell = |decision, expires_at| {
            ApprovalRule::new(
                ToolMatcher { tool: "shell".to_string(), arguments: Default::default() },
                decision,
                Reach::Everywhere,
                expires_at,
            )
            .unwrap()
        };
        let allow = shell(RuleDecision::Allow, None);
        profile.add_approval_rule(allow.clone()).unwrap();
        assert_eq!(
            profile.add_approval_rule(shell(RuleDecision::Ask, None)),
            Err(RuleError::NotAnAnswer),
            "asking is no answer"
        );
        let now = Utc::now();
        profile.add_approval_rule(shell(RuleDecision::Deny, Some(now))).unwrap();
        profile.prune_approval_rules(now);
        assert_eq!(profile.approval_rules(), slice::from_ref(&allow), "what expired is dropped");

        let stored: UserProfile =
            serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();
        assert_eq!(stored.approval_rules(), slice::from_ref(&allow), "kept with the profile");
        assert!(profile.remove_approval_rule(allow.id));
        assert!(!profile.remove_approval_rule(allow.id), "once");
        for _ in 0..MAX_PERSONAL_RULES {
            profile.add_approval_rule(shell(RuleDecision::Allow, None)).unwrap();
        }
        assert_eq!(
            profile.add_approval_rule(shell(RuleDecision::Allow, None)),
            Err(RuleError::TooMany)
        );
    }

    #[tokio::test]
    async fn a_user_writes_their_own_profile_and_no_one_else_s() {
        use crate::channel::ChannelId;
        use crate::security::{OwnershipPolicy, SecurityContext};
        use crate::session::SessionId;
        use crate::store::Persistence;
        use crate::store::StoreError;
        use std::sync::Arc;

        let store = Persistence::in_memory_with(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .user_profiles;
        let (alice, bob) = (UserId::new(), UserId::new());
        let as_user = |id| {
            SecurityContext::new(ActorId::User(id), SessionId::default(), ChannelId::default())
        };

        let hers = UserProfile::new(alice, Some(Introduction::new("I run the budget.").unwrap()));
        assert!(
            matches!(
                store.create(&as_user(bob), &hers).await,
                Err(StoreError::PermissionDenied(_))
            ),
            "nobody writes a profile of someone else"
        );
        store.create(&as_user(alice), &hers).await.expect("her own");
        let changed =
            UserProfile::new(alice, Some(Introduction::new("Bob decides everything.").unwrap()));
        assert!(
            matches!(store.update(&as_user(bob), &changed).await, Err(StoreError::NotFound { .. })),
            "nor changes it"
        );
        assert_eq!(
            store
                .get(&SecurityContext::system(), alice)
                .await
                .unwrap()
                .introduction
                .as_ref()
                .map(Introduction::as_str),
            Some("I run the budget.")
        );
    }
}
