//! `Scope`: where a definition lies (issue 177), and finding a definition
//! by name from a place (issue 178).
//!
//! A place is where one looks from - a channel, a session - and who is
//! looking. From it, a definition is found in the place's own scope, in
//! the scopes around it and in the looking user's own; of several of one
//! name, the one that lies innermost. The store lists what the caller may
//! read; this only chooses among it, so it never widens what anybody
//! reads.

use crate::session::{ChannelId, SessionId};
use crate::user::UserId;
use crate::value::name::{Name, ScopedName};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use uuid::Uuid;

/// Where a definition - a personality, a session template, an agent - lies:
/// the name space it is found in by name (issue 177). Sessions, channels and
/// users have none: they are scopes themselves.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// A channel, by id: a renamed channel keeps what lies in it (issue
    /// 178).
    ChannelScope { session_id: SessionId, channel_id: ChannelId },
    SessionScope { session_id: SessionId },
    UserScope { user_id: UserId },
    /// Not yet a place anything is looked up from (issue 23): what lies in
    /// it is found from nowhere.
    GroupScope { group_id: Uuid },
    #[default]
    GlobalScope
}

impl Scope {
    /// Whether `other` is this scope or lies within it.
    pub fn contains(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::GlobalScope, _) => true,
            (Scope::GroupScope { group_id: id1 }, Scope::GroupScope { group_id: id2 }) => id1 == id2,
            (Scope::UserScope { user_id: id1 }, Scope::UserScope { user_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::SessionScope { session_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::ChannelScope { session_id: id2, .. }) => id1 == id2,
            (Scope::ChannelScope { session_id: s1, channel_id: c1 }, Scope::ChannelScope { session_id: s2, channel_id: c2 }) => s1 == s2 && c1 == c2,
            _ => false,
        }
    }

    /// How far in it lies: what a lookup prefers, the innermost first.
    fn depth(&self) -> u8 {
        match self {
            Scope::GlobalScope => 0,
            Scope::UserScope { .. } | Scope::GroupScope { .. } => 1,
            Scope::SessionScope { .. } => 2,
            Scope::ChannelScope { .. } => 3,
        }
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

/// What lies in a scope and is found there by name: a personality, a
/// session template, an agent.
pub trait Definition {
    fn scoped_name(&self) -> &ScopedName;
}

/// The definition of `name` that `user` finds from `place` among
/// `definitions` - the one that lies innermost; of two that lie equally
/// deep, the first. `None` if none of them is visible from there. Names
/// are compared as `Name` compares them (issue 248).
pub fn find_by_name<'a, D: Definition>(definitions: impl IntoIterator<Item = &'a D>, name: &Name, place: &Scope, user: Option<UserId>) -> Option<&'a D> {
    definitions.into_iter()
        .filter(|d| d.scoped_name().name == *name && d.scoped_name().scope.visible_from(place, user))
        .min_by_key(|d| Reverse(d.scoped_name().scope.depth()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_scope_contains_all() {
        let global = Scope::GlobalScope;
        assert!(global.contains(&Scope::GlobalScope));
        assert!(global.contains(&Scope::GroupScope { group_id: Uuid::new_v4() }));
        assert!(global.contains(&Scope::UserScope { user_id: UserId::new() }));
        assert!(global.contains(&Scope::SessionScope { session_id: SessionId::new() }));
        assert!(global.contains(&Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }));
    }

    #[test]
    fn test_group_scope_contains() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
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

    /// A definition, told apart by `what`.
    struct Defined {
        name: ScopedName,
        what: &'static str,
    }

    impl Definition for Defined {
        fn scoped_name(&self) -> &ScopedName {
            &self.name
        }
    }

    fn defined(name: &str, scope: Scope, what: &'static str) -> Defined {
        Defined { name: ScopedName::new(name, scope).unwrap(), what }
    }

    fn found(definitions: &[Defined], name: &str, place: &Scope, user: Option<UserId>) -> Option<&'static str> {
        find_by_name(definitions, &Name::new(name).unwrap(), place, user).map(|d| d.what)
    }

    #[test]
    fn a_definition_is_found_innermost_first_from_a_place() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let session_id = SessionId::new();
        let (channel_id, other_channel) = (ChannelId::new(), ChannelId::new());
        let channel = Scope::ChannelScope { session_id, channel_id };
        let session = Scope::SessionScope { session_id };
        let definitions = [
            defined("helper", Scope::GlobalScope, "global"),
            defined("helper", Scope::UserScope { user_id: alice }, "alice's"),
            defined("helper", session.clone(), "the session's"),
            defined("helper", channel.clone(), "the channel's"),
        ];

        assert_eq!(found(&definitions, "helper", &channel, Some(alice)), Some("the channel's"));
        let next_door = Scope::ChannelScope { session_id, channel_id: other_channel };
        assert_eq!(found(&definitions, "helper", &next_door, Some(alice)), Some("the session's"), "not another channel's");
        assert_eq!(found(&definitions, "helper", &Scope::SessionScope { session_id: SessionId::new() }, Some(alice)), Some("alice's"), "from another session, her own");
        assert_eq!(found(&definitions, "helper", &Scope::SessionScope { session_id: SessionId::new() }, Some(bob)), Some("global"), "not somebody else's");
        assert_eq!(found(&definitions, "helper", &Scope::SessionScope { session_id: SessionId::new() }, None), Some("global"), "nobody's own");
        assert_eq!(found(&definitions, "HELPER", &channel, Some(alice)), Some("the channel's"), "names compare as names do");
        assert_eq!(found(&definitions, "other", &channel, Some(alice)), None);
    }

    #[test]
    fn what_lies_in_a_scope_not_around_the_place_is_not_found() {
        let session = Scope::SessionScope { session_id: SessionId::new() };
        let definitions = [
            defined("helper", Scope::SessionScope { session_id: SessionId::new() }, "another session's"),
            defined("helper", Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }, "another channel's"),
            defined("helper", Scope::GroupScope { group_id: Uuid::new_v4() }, "a group's"),
        ];
        assert_eq!(found(&definitions, "helper", &session, Some(UserId::new())), None);

        let group = Scope::GroupScope { group_id: Uuid::new_v4() };
        let in_group = [defined("helper", group.clone(), "the group's")];
        assert_eq!(found(&in_group, "helper", &group, None), None, "a group is no place yet (issue 23)");
    }

    #[test]
    fn of_two_equally_deep_the_first_is_found() {
        let definitions = [defined("helper", Scope::GlobalScope, "first"), defined("Helper", Scope::GlobalScope, "second")];
        assert_eq!(found(&definitions, "helper", &Scope::GlobalScope, None), Some("first"));
    }
}
