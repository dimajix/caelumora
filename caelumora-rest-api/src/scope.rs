//! Where something lies that is kept in a scope: a workspace, a skill, an artifact, a group.

use caelumora_model::scope::Scope;
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};

/// Where a workspace (issue 251, decision 13), a skill (issue 22) or an
/// artifact (issue 271) lies: a session's scope, its owner's or the global
/// one. One of a session's scope is made with the session - a workspace by
/// its template or as chosen when it is made (issue 266), a skill by an
/// agent in it, an artifact by publishing it -, not on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Session,
    /// Its owner's: found by its name among theirs.
    #[default]
    User,
    /// The global one: its name unique among all.
    Global,
}

impl ScopeKind {
    /// The kind of `scope`, and the session it names if it is a session's.
    /// A workspace, a skill and an artifact lie in no other than these three
    /// (`Stored::validate`).
    pub fn of(scope: &Scope) -> (Self, Option<SessionId>) {
        match scope {
            Scope::SessionScope { session_id } => (ScopeKind::Session, Some(*session_id)),
            Scope::UserScope { .. } => (ScopeKind::User, None),
            _ => (ScopeKind::Global, None),
        }
    }

    /// The scope of `user`'s own thing put here on its own: theirs or the
    /// global one; `None` for a session's, which nothing is put into so.
    pub fn of_user(self, user: UserId) -> Option<Scope> {
        match self {
            Self::User => Some(Scope::UserScope { user_id: user }),
            Self::Global => Some(Scope::GlobalScope),
            Self::Session => None,
        }
    }
}

/// What a workspace holds, or what an artifact is (issue 271), replacing
/// what it said: `null` or empty removes it.
#[derive(Debug, Deserialize)]
pub struct ItemDescriptionRequest {
    #[serde(default)]
    pub description: Option<String>,
}
