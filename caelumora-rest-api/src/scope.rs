//! Where something lies that is kept in a scope: a workspace, a skill, a group, an agent, a
//! personality, a project (issue 467).

use caelumora_model::project::ProjectId;
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::ScopeSnapshot;
use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};

/// The kind of a scope, as a request names it and `ScopeResponse` answers it: where a workspace
/// (issue 251, decision 13), a skill (issue 22), a project (issue 467), a group, an agent or a
/// personality lies - a session's scope, its owner's or the global one. One of a session's scope
/// is made with the session - a workspace by its template or as chosen when it is made (issue
/// 266), a skill by an agent in it, the session's own project -, not on its own. The members of a
/// project lie in its scope (`project`).
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

/// Where something kept in a scope lies, as a response answers it: the kind of its scope and what
/// that scope names - its session, its channel, its project, its user and their name where it is
/// known (issue 475), who owns it. A workspace, a skill and a project lie in no scope of a channel
/// or a project (`Stored::validate`); a group's scope is no place anything lies in yet, and is
/// answered as the global one.
#[derive(Debug, Serialize)]
pub struct ScopeResponse {
    #[serde(rename = "scope")]
    pub kind: ScopeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
}

impl ScopeResponse {
    /// Where `snapshot` lies, as a response answers it.
    pub fn of(snapshot: &ScopeSnapshot) -> Self {
        let bare = |kind| Self {
            kind,
            session_id: None,
            channel_id: None,
            project_id: None,
            user_id: None,
            user_name: None,
        };
        match &snapshot.scope {
            Scope::SessionScope { session_id } => {
                Self { session_id: Some(*session_id), ..bare(ScopeKind::Session) }
            }
            Scope::ChannelScope { session_id, channel_id } => Self {
                session_id: Some(*session_id),
                channel_id: Some(*channel_id),
                ..bare(ScopeKind::Session)
            },
            Scope::ProjectScope { project_id } => {
                Self { project_id: Some(*project_id), ..bare(ScopeKind::Project) }
            }
            Scope::UserScope { user_id } => Self {
                user_id: Some(*user_id),
                user_name: snapshot.user_name.clone(),
                ..bare(ScopeKind::User)
            },
            Scope::GlobalScope | Scope::GroupScope { .. } => bare(ScopeKind::Global),
        }
    }
}

impl ScopeKind {
    /// The scope a request names with this kind, `session_id` and
    /// `channel_id`, as `ScopeResponse` answers them, for something of `user`'s:
    /// `None` if they do not fit - a session's without its session, a
    /// session or channel named for another kind.
    pub fn place(
        self,
        user: UserId,
        session_id: Option<SessionId>,
        channel_id: Option<ChannelId>,
    ) -> Option<Scope> {
        match (self, session_id, channel_id) {
            (Self::Session, Some(session_id), None) => Some(Scope::SessionScope { session_id }),
            (Self::Session, Some(session_id), Some(channel_id)) => {
                Some(Scope::ChannelScope { session_id, channel_id })
            }
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
    use caelumora_model::group::GroupId;

    /// `scope` as shown to a viewer who knows no name of its user.
    fn unnamed(scope: Scope) -> ScopeSnapshot {
        ScopeSnapshot { scope, user_name: None }
    }

    /// A request names a scope as a response does: a session's with its
    /// session, a channel's with its channel besides; nothing else names
    /// one.
    #[test]
    fn a_scope_is_placed_as_it_is_answered() {
        let user = UserId::new();
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        for scope in [
            Scope::UserScope { user_id: user },
            Scope::GlobalScope,
            Scope::SessionScope { session_id },
            Scope::ChannelScope { session_id, channel_id },
        ] {
            let answered = ScopeResponse::of(&unnamed(scope.clone()));
            assert_eq!(
                answered.kind.place(user, answered.session_id, answered.channel_id),
                Some(scope)
            );
        }
        assert_eq!(
            ScopeKind::Session.place(user, None, None),
            None,
            "a session's without its session"
        );
        assert_eq!(ScopeKind::Session.place(user, None, Some(channel_id)), None);
        assert_eq!(
            ScopeKind::User.place(user, Some(session_id), None),
            None,
            "a session named for another kind"
        );
        assert_eq!(ScopeKind::Global.place(user, None, Some(channel_id)), None);
    }

    /// The place is answered beside what lies there, naming only what its scope names - a user's
    /// its user, and their name where it is known (issue 475).
    #[test]
    fn a_scope_is_answered_by_its_kind_and_what_it_names() {
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        let user_id = UserId::new();
        let answered = |snapshot| serde_json::to_value(ScopeResponse::of(&snapshot)).unwrap();
        let theirs = ScopeSnapshot {
            scope: Scope::UserScope { user_id },
            user_name: Some("bob".to_string()),
        };
        assert_eq!(
            answered(theirs),
            serde_json::json!({"scope": "user", "user_id": user_id, "user_name": "bob"})
        );
        assert_eq!(
            answered(unnamed(Scope::UserScope { user_id })),
            serde_json::json!({"scope": "user", "user_id": user_id}),
            "a user no longer known"
        );
        assert_eq!(
            answered(unnamed(Scope::GroupScope { group_id: GroupId::new() })),
            serde_json::json!({"scope": "global"}),
            "no place anything lies in yet"
        );
        assert_eq!(
            answered(unnamed(Scope::ChannelScope { session_id, channel_id })),
            serde_json::json!({"scope": "session", "session_id": session_id, "channel_id": channel_id})
        );
    }

    /// Issue 467: the scope of a project is where its members lie, and nothing is put there by a
    /// request.
    #[test]
    fn a_project_s_scope_is_shown_and_placed_nothing_in() {
        let user = UserId::new();
        let project_id = ProjectId::new();
        let answered =
            serde_json::to_value(ScopeResponse::of(&unnamed(Scope::ProjectScope { project_id })))
                .unwrap();
        assert_eq!(answered, serde_json::json!({"scope": "project", "project_id": project_id}));
        assert_eq!(ScopeKind::Project.of_user(user), None);
        assert_eq!(ScopeKind::Project.place(user, None, None), None);
    }
}
