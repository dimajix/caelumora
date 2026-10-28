//! `Scope`: where a personality, a session template or an agent lies
//! (issue 177), and finding one by name from a place (issue 178) - and
//! what is `Scoped`: what lies in a scope under a name that is unique there
//! (issue 250).
//!
//! A place is where one looks from - a channel, a session - and who is
//! looking. From it, what is `Scoped` is found in the place's own scope, in
//! the scopes around it and in the looking user's own; of several of one
//! name, the one that lies innermost. The store lists what the caller may
//! read; this only chooses among it, so it never widens what anybody
//! reads.

use crate::group::GroupId;
use crate::project::ProjectId;
use crate::session::{ChannelId, SessionId};
use crate::user::UserId;
use crate::value::name::{Name, ScopedName};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use crate::store::query::{Field, FieldValue, Filter};

/// Where something lies that is found by name (issue 177): the name space of
/// a personality, a session template, an agent, a workspace, a skill, a
/// project or an artifact, each stored with its scope. A channel's name space
/// is its session's scope, a user's the global one (issue 250); neither
/// stores a scope of its own. A project lies in a scope and holds one, where
/// its artifacts lie (issue 467).
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// A channel, by id: a renamed channel keeps what lies in it (issue
    /// 178).
    ChannelScope { session_id: SessionId, channel_id: ChannelId },
    SessionScope { session_id: SessionId },
    /// A project's (issue 467): what lies in it goes with the project. It
    /// contains nothing but itself: a session in a project is told by the
    /// session, not by its scope, since sessions change projects and scopes
    /// are stored.
    ProjectScope { project_id: ProjectId },
    UserScope { user_id: UserId },
    /// Not yet a place anything is looked up from (issue 23): what lies in
    /// it is found from nowhere.
    GroupScope { group_id: GroupId },
    #[default]
    GlobalScope
}

impl Scope {
    /// The project whose scope this is, if it is one (issue 467).
    pub fn project(&self) -> Option<ProjectId> {
        match self {
            Scope::ProjectScope { project_id } => Some(*project_id),
            Scope::GlobalScope | Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::SessionScope { .. } | Scope::ChannelScope { .. } => None,
        }
    }

    /// Whether `other` is this scope or lies within it.
    pub fn contains(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::GlobalScope, _) => true,
            (Scope::GroupScope { group_id: id1 }, Scope::GroupScope { group_id: id2 }) => id1 == id2,
            (Scope::UserScope { user_id: id1 }, Scope::UserScope { user_id: id2 }) => id1 == id2,
            (Scope::ProjectScope { project_id: id1 }, Scope::ProjectScope { project_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::SessionScope { session_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::ChannelScope { session_id: id2, .. }) => id1 == id2,
            (Scope::ChannelScope { session_id: s1, channel_id: c1 }, Scope::ChannelScope { session_id: s2, channel_id: c2 }) => s1 == s2 && c1 == c2,
            _ => false,
        }
    }

    /// Whether what lies in this scope may be used at `place` - an agent
    /// take part in a channel there, a personality make an agent there: what
    /// lies in a session's or a channel's scope only within it, since it goes
    /// with it (issue 178); what lies in a user's or a group's scope or
    /// everywhere wherever the permissions allow - that scope says only
    /// where its name is found. What lies in a project's scope is used
    /// nowhere yet (issue 467): whether a place is in the project, the
    /// place's session says, not its scope.
    pub fn admits(&self, place: &Scope) -> bool {
        match self {
            Scope::SessionScope { .. } | Scope::ChannelScope { .. } => self.contains(place),
            Scope::ProjectScope { .. } => false,
            Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::GlobalScope => true,
        }
    }

    /// The scope as one text that tells it from every other (issue 447): its
    /// variant and its ids - what the key of a name in it begins with
    /// (`ScopedName::key`).
    pub(crate) fn key(&self) -> String {
        match self {
            Scope::ChannelScope { session_id, channel_id } => format!("channel:{session_id}:{channel_id}"),
            Scope::SessionScope { session_id } => format!("session:{session_id}"),
            Scope::ProjectScope { project_id } => format!("project:{project_id}"),
            Scope::UserScope { user_id } => format!("user:{user_id}"),
            Scope::GroupScope { group_id } => format!("group:{group_id}"),
            Scope::GlobalScope => "global".to_string(),
        }
    }

    /// How far in it lies: what a lookup prefers, the innermost first.
    fn depth(&self) -> u8 {
        match self {
            Scope::GlobalScope => 0,
            Scope::UserScope { .. } | Scope::GroupScope { .. } => 1,
            Scope::ProjectScope { .. } => 2,
            Scope::SessionScope { .. } => 3,
            Scope::ChannelScope { .. } => 4,
        }
    }

    /// This scope and every one that contains it, the innermost first: what
    /// lies in any of them is found from here, as `contains` says.
    fn holders(&self) -> Vec<Scope> {
        let mut holders = vec![self.clone()];
        if let Scope::ChannelScope { session_id, .. } = self {
            holders.push(Scope::SessionScope { session_id: *session_id });
        }
        if *self != Scope::GlobalScope {
            holders.push(Scope::GlobalScope);
        }
        holders
    }

    /// Whether what lies in this scope is found from `place` by `user`: it
    /// is the place's scope or one around it, or the user's own.
    fn visible_from(&self, place: &Scope, user: Option<UserId>) -> bool {
        match self {
            Scope::GroupScope { .. } => false,
            Scope::UserScope { user_id } if Some(*user_id) == user => true,
            _ => self.contains(place),
        }
    }
}

/// The field of a kind that lies in a scope, as a query names it (issue
/// 293): its scope.
pub trait ScopedField: Field {
    const SCOPE: Self;
}

/// The fields of a kind that a list asks by where it lies and what it is
/// called: personalities, session templates, agents, workspaces, skills,
/// projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeField {
    Scope,
    /// Its name within its scope (issue 221).
    Name,
}

impl Field for ScopeField {
    fn path(self) -> &'static [&'static str] {
        match self {
            ScopeField::Scope => &["scope"],
            ScopeField::Name => &["name"],
        }
    }
}

impl ScopedField for ScopeField {
    const SCOPE: Self = ScopeField::Scope;
}

/// The conditions a list of what lies in a scope is asked by (issue 293).
impl<F: ScopedField> Filter<F> {
    /// What lies in `scope` or within it, as `Scope::contains` says -
    /// everything for the global scope, which holds everything. Within a
    /// session's scope lie the scopes of its channels, which hold their
    /// session as well.
    pub fn lying_within(scope: &Scope) -> Self {
        match scope {
            Scope::GlobalScope => Filter::all_of([]),
            Scope::SessionScope { session_id } => Filter::is_in(F::SCOPE, [
                FieldValue::from(scope),
                FieldValue::tagged("ChannelScope", FieldValue::tagged("session_id", *session_id)),
            ]),
            other => Filter::eq(F::SCOPE, other),
        }
    }

    /// What is seen from `place` by `user`, as `Scope::visible_from` says:
    /// what lies in a scope that holds `place`, and in the user's own.
    pub fn seen_from(place: &Scope, user: Option<UserId>) -> Self {
        let mut seen: Vec<Scope> = place.holders().into_iter().filter(|scope| !matches!(scope, Scope::GroupScope { .. })).collect();
        if let Some(user_id) = user {
            let own = Scope::UserScope { user_id };
            if !seen.contains(&own) {
                seen.push(own);
            }
        }
        Filter::is_in(F::SCOPE, &seen)
    }
}

/// A scope as a query compares it, after serde's tags: by what identifies
/// it - a channel's scope by its channel alone, whose id is unique.
impl From<&Scope> for FieldValue {
    fn from(scope: &Scope) -> Self {
        match scope {
            Scope::ChannelScope { channel_id, .. } => FieldValue::tagged("ChannelScope", FieldValue::tagged("channel_id", *channel_id)),
            Scope::SessionScope { session_id } => FieldValue::tagged("SessionScope", FieldValue::tagged("session_id", *session_id)),
            Scope::ProjectScope { project_id } => FieldValue::tagged("ProjectScope", FieldValue::tagged("project_id", *project_id)),
            Scope::UserScope { user_id } => FieldValue::tagged("UserScope", FieldValue::tagged("user_id", *user_id)),
            Scope::GroupScope { group_id } => FieldValue::tagged("GroupScope", FieldValue::tagged("group_id", *group_id)),
            Scope::GlobalScope => FieldValue::from("GlobalScope"),
        }
    }
}

impl From<Scope> for FieldValue {
    fn from(scope: Scope) -> Self {
        FieldValue::from(&scope)
    }
}

/// What lies in a scope and is found there by its name, which is unique of
/// its kind in that scope (issue 250): a personality, a session template,
/// an agent - and a channel, whose scope is its session, a user, who lies
/// in the global one, a workspace, which lies in its owner's, and an artifact, which lies in its
/// project's (issue 467). A kind is either `Scoped` or has no unique name at all.
pub trait Scoped {
    /// Its name and the scope it lies in; made when asked by a kind that
    /// does not store it so.
    fn scoped_name(&self) -> ScopedName;
}

/// The one of `name` that `user` finds from `place` among `scoped` -
/// the one that lies innermost; of two that lie equally
/// deep, the first. `None` if none of them is visible from there. Names
/// are compared as `Name` compares them (issue 248).
pub fn find_by_name<'a, D: Scoped>(scoped: impl IntoIterator<Item = &'a D>, name: &Name, place: &Scope, user: Option<UserId>) -> Option<&'a D> {
    scoped.into_iter()
        .map(|d| (d.scoped_name(), d))
        .filter(|(scoped_name, _)| scoped_name.name == *name && scoped_name.scope.visible_from(place, user))
        .min_by_key(|(scoped_name, _)| Reverse(scoped_name.scope.depth()))
        .map(|(_, d)| d)
}

/// What `user` finds from `place` among `scoped`, by name: for every name
/// visible from there the one `find_by_name` finds - what a catalogue of
/// them lists (issue 22). In the order of their names.
pub fn found_from<'a, D: Scoped>(scoped: impl IntoIterator<Item = &'a D>, place: &Scope, user: Option<UserId>) -> Vec<&'a D> {
    let visible: Vec<(ScopedName, &D)> = scoped.into_iter()
        .map(|d| (d.scoped_name(), d))
        .filter(|(scoped_name, _)| scoped_name.scope.visible_from(place, user))
        .collect();
    let mut found: Vec<(ScopedName, &D)> = Vec::new();
    for (scoped_name, d) in visible {
        match found.iter_mut().find(|(other, _)| other.name == scoped_name.name) {
            Some(entry) if scoped_name.scope.depth() > entry.0.scope.depth() => *entry = (scoped_name, d),
            Some(_) => {}
            None => found.push((scoped_name, d)),
        }
    }
    found.sort_by(|(l, _), (r, _)| l.name.as_str().cmp(r.name.as_str()));
    found.into_iter().map(|(_, d)| d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use crate::store::query::meets_exactly_its_equals;

    #[test]
    fn test_global_scope_contains_all() {
        let global = Scope::GlobalScope;
        assert!(global.contains(&Scope::GlobalScope));
        assert!(global.contains(&Scope::GroupScope { group_id: GroupId::new() }));
        assert!(global.contains(&Scope::UserScope { user_id: UserId::new() }));
        assert!(global.contains(&Scope::SessionScope { session_id: SessionId::new() }));
        assert!(global.contains(&Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }));
    }

    #[test]
    fn test_group_scope_contains() {
        let id1 = GroupId::new();
        let id2 = GroupId::new();
        let group1 = Scope::GroupScope { group_id: id1 };
        
        assert!(group1.contains(&Scope::GroupScope { group_id: id1 }));
        assert!(!group1.contains(&Scope::GroupScope { group_id: id2 }));
        assert!(!group1.contains(&Scope::GlobalScope));
    }

    #[test]
    fn test_user_scope_contains() {
        let id1 = UserId::new();
        let id2 = UserId::new();
        let user1 = Scope::UserScope { user_id: id1 };
        
        assert!(user1.contains(&Scope::UserScope { user_id: id1 }));
        assert!(!user1.contains(&Scope::UserScope { user_id: id2 }));
        assert!(!user1.contains(&Scope::GlobalScope));
    }

    #[test]
    fn test_session_scope_contains() {
        let s1 = SessionId::new();
        let s2 = SessionId::new();
        let session1 = Scope::SessionScope { session_id: s1 };
        
        assert!(session1.contains(&Scope::SessionScope { session_id: s1 }));
        assert!(!session1.contains(&Scope::SessionScope { session_id: s2 }));
        assert!(session1.contains(&Scope::ChannelScope { session_id: s1, channel_id: ChannelId::new() }));
        assert!(!session1.contains(&Scope::ChannelScope { session_id: s2, channel_id: ChannelId::new() }));
        assert!(!session1.contains(&Scope::GlobalScope));
    }

    /// A project's scope contains nothing but itself (issue 467): a session
    /// in the project is told by the session.
    #[test]
    fn a_project_scope_contains_itself_alone() {
        let project_id = ProjectId::new();
        let project = Scope::ProjectScope { project_id };
        assert_eq!((project.project(), Scope::GlobalScope.project()), (Some(project_id), None), "the project whose it is");
        assert!(project.contains(&Scope::ProjectScope { project_id }));
        assert!(!project.contains(&Scope::ProjectScope { project_id: ProjectId::new() }));
        let session_id = SessionId::new();
        for other in [Scope::SessionScope { session_id }, Scope::ChannelScope { session_id, channel_id: ChannelId::new() }, Scope::GlobalScope] {
            assert!(!project.contains(&other), "{other:?}");
        }
        assert!(Scope::GlobalScope.contains(&project));
    }

    #[test]
    fn test_channel_scope_contains() {
        let s1 = SessionId::new();
        let s2 = SessionId::new();
        let c1 = ChannelId::new();
        let c2 = ChannelId::new();
        let channel_scope = Scope::ChannelScope { session_id: s1, channel_id: c1 };

        assert!(channel_scope.contains(&Scope::ChannelScope { session_id: s1, channel_id: c1 }));
        assert!(!channel_scope.contains(&Scope::ChannelScope { session_id: s1, channel_id: c2 }));
        assert!(!channel_scope.contains(&Scope::ChannelScope { session_id: s2, channel_id: c1 }));
        assert!(!channel_scope.contains(&Scope::SessionScope { session_id: s1 }));
        assert!(!channel_scope.contains(&Scope::GlobalScope));
    }

    /// Something in a scope, told apart by `what`.
    struct Item {
        name: ScopedName,
        what: &'static str,
    }

    impl Scoped for Item {
        fn scoped_name(&self) -> ScopedName {
            self.name.clone()
        }
    }

    fn item(name: &str, scope: Scope, what: &'static str) -> Item {
        Item { name: ScopedName::new(name, scope).unwrap(), what }
    }

    fn found(items: &[Item], name: &str, place: &Scope, user: Option<UserId>) -> Option<&'static str> {
        find_by_name(items, &Name::new(name).unwrap(), place, user).map(|d| d.what)
    }

    #[test]
    fn the_innermost_of_a_name_is_found_from_a_place() {
        let (alice, bob) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));
        let session_id = SessionId::new();
        let (channel_id, other_channel) = (ChannelId::new(), ChannelId::new());
        let channel = Scope::ChannelScope { session_id, channel_id };
        let session = Scope::SessionScope { session_id };
        let items = [
            item("helper", Scope::GlobalScope, "global"),
            item("helper", Scope::UserScope { user_id: alice }, "alice's"),
            item("helper", session.clone(), "the session's"),
            item("helper", channel.clone(), "the channel's"),
        ];

        assert_eq!(found(&items, "helper", &channel, Some(alice)), Some("the channel's"));
        let next_door = Scope::ChannelScope { session_id, channel_id: other_channel };
        assert_eq!(found(&items, "helper", &next_door, Some(alice)), Some("the session's"), "not another channel's");
        assert_eq!(found(&items, "helper", &Scope::SessionScope { session_id: SessionId::new() }, Some(alice)), Some("alice's"), "from another session, her own");
        assert_eq!(found(&items, "helper", &Scope::SessionScope { session_id: SessionId::new() }, Some(bob)), Some("global"), "not somebody else's");
        assert_eq!(found(&items, "helper", &Scope::SessionScope { session_id: SessionId::new() }, None), Some("global"), "nobody's own");
        assert_eq!(found(&items, "HELPER", &channel, Some(alice)), Some("the channel's"), "names compare as names do");
        assert_eq!(found(&items, "other", &channel, Some(alice)), None);
    }

    #[test]
    fn what_lies_in_a_scope_not_around_the_place_is_not_found() {
        let session = Scope::SessionScope { session_id: SessionId::new() };
        let items = [
            item("helper", Scope::SessionScope { session_id: SessionId::new() }, "another session's"),
            item("helper", Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }, "another channel's"),
            item("helper", Scope::GroupScope { group_id: GroupId::new() }, "a group's"),
            item("helper", Scope::ProjectScope { project_id: ProjectId::new() }, "a project's, which the session's scope does not know"),
        ];
        assert_eq!(found(&items, "helper", &session, Some(UserId::new())), None);

        let group = Scope::GroupScope { group_id: GroupId::new() };
        let in_group = [item("helper", group.clone(), "the group's")];
        assert_eq!(found(&in_group, "helper", &group, None), None, "a group is no place yet (issue 23)");
    }

    #[test]
    fn what_lies_in_a_session_or_a_channel_is_used_only_within_it() {
        let session_id = SessionId::new();
        let (channel_id, other_channel) = (ChannelId::new(), ChannelId::new());
        let channel = Scope::ChannelScope { session_id, channel_id };
        let session = Scope::SessionScope { session_id };
        let elsewhere = Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() };

        assert!(session.admits(&channel) && session.admits(&session), "a session's in it and its channels");
        assert!(!session.admits(&elsewhere), "not in another session");
        assert!(channel.admits(&channel), "a channel's in it");
        assert!(!channel.admits(&Scope::ChannelScope { session_id, channel_id: other_channel }), "not in another channel of its session");
        assert!(!channel.admits(&session), "nor in its session at large");
        let project = Scope::ProjectScope { project_id: ProjectId::new() };
        for place in [&channel, &session, &project] {
            assert!(!project.admits(place), "what lies in a project's is used nowhere yet (issue 467): {place:?}");
        }
        for anywhere in [Scope::UserScope { user_id: UserId::new() }, Scope::GroupScope { group_id: GroupId::new() }, Scope::GlobalScope] {
            assert!(anywhere.admits(&elsewhere), "{anywhere:?} only says where its name is found");
        }
    }

    #[test]
    fn of_two_equally_deep_the_first_is_found() {
        let items = [item("helper", Scope::GlobalScope, "first"), item("Helper", Scope::GlobalScope, "second")];
        assert_eq!(found(&items, "helper", &Scope::GlobalScope, None), Some("first"));
    }

    #[test]
    fn what_is_found_from_a_place_is_the_innermost_of_every_name() {
        let (alice, session_id) = (UserId::new(), SessionId::new());
        let place = Scope::ChannelScope { session_id, channel_id: ChannelId::new() };
        let items = [
            item("notes", Scope::GlobalScope, "the global notes"),
            item("notes", Scope::SessionScope { session_id }, "the session's notes"),
            item("deploy", Scope::UserScope { user_id: alice }, "alice's deploy"),
            item("audit", Scope::SessionScope { session_id: SessionId::new() }, "another session's"),
            item("audit", Scope::UserScope { user_id: UserId::new() }, "another user's"),
        ];
        let listed: Vec<&str> = found_from(&items, &place, Some(alice)).into_iter().map(|d| d.what).collect();
        assert_eq!(listed, ["alice's deploy", "the session's notes"], "by name, each the innermost; nothing of another session or user");
        let as_nobody: Vec<&str> = found_from(&items, &place, None).into_iter().map(|d| d.what).collect();
        assert_eq!(as_nobody, ["the session's notes"]);
    }

    /// The conditions a list is asked with (issue 293) meet exactly what
    /// `contains` and `visible_from` say, over every pair of scopes - no
    /// two of which share a channel, whose id is unique.
    #[test]
    fn the_conditions_of_a_list_say_what_contains_and_visible_from_say() {
        let (alice, bob) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));
        let (s1, s2) = (SessionId(Uuid::new_v4()), SessionId(Uuid::new_v4()));
        let (c1, c2) = (ChannelId(Uuid::new_v4()), ChannelId(Uuid::new_v4()));
        let scopes = [
            Scope::GlobalScope, Scope::UserScope { user_id: alice }, Scope::UserScope { user_id: bob },
            Scope::SessionScope { session_id: s1 }, Scope::SessionScope { session_id: s2 },
            Scope::ChannelScope { session_id: s1, channel_id: c1 }, Scope::ChannelScope { session_id: s1, channel_id: c2 },
            Scope::ChannelScope { session_id: s2, channel_id: ChannelId(Uuid::new_v4()) }, Scope::GroupScope { group_id: GroupId::new() },
            Scope::ProjectScope { project_id: ProjectId(Uuid::new_v4()) }, Scope::ProjectScope { project_id: ProjectId(Uuid::new_v4()) },
        ];
        let document = |scope: &Scope| serde_json::json!({ "scope": scope });
        for holder in &scopes {
            let within = Filter::<ScopeField>::lying_within(holder);
            for scope in &scopes {
                assert_eq!(within.matches(&document(scope)), holder.contains(scope), "{scope:?} within {holder:?}");
            }
        }
        for place in &scopes {
            for user in [None, Some(alice)] {
                let seen = Filter::<ScopeField>::seen_from(place, user);
                for scope in &scopes {
                    assert_eq!(seen.matches(&document(scope)), scope.visible_from(place, user), "{scope:?} seen from {place:?} by {user:?}");
                }
            }
        }
    }


    /// A scope as a query compares it (issue 293) - a channel's by its
    /// channel alone, whose id is unique, so that no two samples share one.
    #[test]
    fn a_scope_meets_exactly_the_scopes_equal_to_it() {
        let id = Uuid::new_v4();
        meets_exactly_its_equals(&[
            Scope::GlobalScope, Scope::UserScope { user_id: UserId(id) }, Scope::GroupScope { group_id: GroupId(id) },
            Scope::ProjectScope { project_id: ProjectId(id) }, Scope::ProjectScope { project_id: ProjectId::new() },
            Scope::SessionScope { session_id: SessionId(id) }, Scope::SessionScope { session_id: SessionId::new() },
            Scope::ChannelScope { session_id: SessionId(id), channel_id: ChannelId(id) },
            Scope::ChannelScope { session_id: SessionId(id), channel_id: ChannelId::new() },
        ]);
    }

}
