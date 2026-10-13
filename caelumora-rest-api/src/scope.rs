//! Where something lies that is kept in a scope: a workspace, a skill, an artifact, a group,
//! an agent, a personality.

use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, SessionId};
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

    /// The kind of `scope` of what may lie in a channel's scope too - a
    /// group, an agent, a personality -, with the session it names and its
    /// channel if it is a channel's: `session` for both.
    pub fn placed(scope: &Scope) -> (Self, Option<SessionId>, Option<ChannelId>) {
        match scope {
            Scope::ChannelScope { session_id, channel_id } => (ScopeKind::Session, Some(*session_id), Some(*channel_id)),
            scope => {
                let (kind, session_id) = Self::of(scope);
                (kind, session_id, None)
            }
        }
    }

    /// The scope a request names with this kind, `session_id` and
    /// `channel_id`, as `placed` answers them, for something of `user`'s:
    /// `None` if they do not fit - a session's without its session, a
    /// session or channel named for another kind.
    pub fn place(self, user: UserId, session_id: Option<SessionId>, channel_id: Option<ChannelId>) -> Option<Scope> {
        match (self, session_id, channel_id) {
            (Self::Session, Some(session_id), None) => Some(Scope::SessionScope { session_id }),
            (Self::Session, Some(session_id), Some(channel_id)) => Some(Scope::ChannelScope { session_id, channel_id }),
            (Self::User | Self::Global, None, None) => self.of_user(user),
            _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A request names a scope as a response does: a session's with its
    /// session, a channel's with its channel besides; nothing else names
    /// one.
    #[test]
    fn a_scope_is_placed_as_it_is_answered() {
        let (user, session_id, channel_id) = (UserId::new(), SessionId::new(), ChannelId::new());
        for scope in [Scope::UserScope { user_id: user }, Scope::GlobalScope, Scope::SessionScope { session_id }, Scope::ChannelScope { session_id, channel_id }] {
            let (kind, session, channel) = ScopeKind::placed(&scope);
            assert_eq!(kind.place(user, session, channel), Some(scope));
        }
        assert_eq!(ScopeKind::Session.place(user, None, None), None, "a session's without its session");
        assert_eq!(ScopeKind::Session.place(user, None, Some(channel_id)), None);
        assert_eq!(ScopeKind::User.place(user, Some(session_id), None), None, "a session named for another kind");
        assert_eq!(ScopeKind::Global.place(user, None, Some(channel_id)), None);
    }
}
