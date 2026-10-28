//! Where something lies that is kept in a scope: a workspace, a skill, a group, an agent, a
//! personality, a project (issue 467).

use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};

/// Where a workspace (issue 251, decision 13), a skill (issue 22) or a
/// project (issue 467) lies: a session's scope, its owner's or the global
/// one. One of a session's scope is made with the session - a workspace by
/// its template or as chosen when it is made (issue 266), a skill by an
/// agent in it, the session's own project -, not on its own. The members of
/// a project lie in its scope (`project`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Session,
    /// Its owner's: found by its name among theirs.
    #[default]
    User,
    /// The global one: its name unique among all.
    Global,
    /// A project's (issue 467): where the group of its members lies. Answered alone: nothing a
    /// request makes lies there.
    #[serde(skip_deserializing)]
    Project,
}

impl ScopeKind {
    /// The kind of `scope`, and the session it names if it is a session's -
    /// a channel's names its session. A workspace, a skill and a project lie
    /// in no other than the first three (`Stored::validate`); a group's scope
    /// is no place anything lies in yet, and is shown as the global one.
    pub fn of(scope: &Scope) -> (Self, Option<SessionId>) {
        match scope {
            Scope::SessionScope { session_id } | Scope::ChannelScope { session_id, .. } => (ScopeKind::Session, Some(*session_id)),
            Scope::UserScope { .. } => (ScopeKind::User, None),
            Scope::ProjectScope { .. } => (ScopeKind::Project, None),
            Scope::GlobalScope | Scope::GroupScope { .. } => (ScopeKind::Global, None),
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
    /// global one; `None` for a session's or a project's, which nothing is
    /// put into so.
    pub fn of_user(self, user: UserId) -> Option<Scope> {
        match self {
            Self::User => Some(Scope::UserScope { user_id: user }),
            Self::Global => Some(Scope::GlobalScope),
            Self::Session | Self::Project => None,
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
    use caelumora_model::project::ProjectId;

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

    /// Issue 467: the scope of a project is where its members lie, and nothing is put there by a
    /// request.
    #[test]
    fn a_project_s_scope_is_shown_and_placed_nothing_in() {
        let user = UserId::new();
        assert_eq!(ScopeKind::of(&Scope::ProjectScope { project_id: ProjectId::new() }), (ScopeKind::Project, None));
        assert_eq!(ScopeKind::Project.of_user(user), None);
        assert_eq!(ScopeKind::Project.place(user, None, None), None);
    }
}
