use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::session::SessionId;
use crate::user::UserId;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    ChannelScope { session_id:SessionId, channel:String },
    SessionScope { session_id:SessionId },
    UserScope { user_id:UserId },
    GroupScope { group_id:Uuid },
    #[default]
    GlobalScope
}

impl Scope {
    pub fn contains(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::GlobalScope, _) => true,
            (Scope::GroupScope { group_id: id1 }, Scope::GroupScope { group_id: id2 }) => id1 == id2,
            (Scope::UserScope { user_id: id1 }, Scope::UserScope { user_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::SessionScope { session_id: id2 }) => id1 == id2,
            (Scope::SessionScope { session_id: id1 }, Scope::ChannelScope { session_id: id2, .. }) => id1 == id2,
            (Scope::ChannelScope { session_id: s1, channel: c1 }, Scope::ChannelScope { session_id: s2, channel: c2 }) => s1 == s2 && c1 == c2,
            _ => false,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub scope: Scope,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use crate::session::SessionId;
    use crate::user::UserId;

    #[test]
    fn test_global_scope_contains_all() {
        let global = Scope::GlobalScope;
        assert!(global.contains(&Scope::GlobalScope));
        assert!(global.contains(&Scope::GroupScope { group_id: Uuid::new_v4() }));
        assert!(global.contains(&Scope::UserScope { user_id: UserId::new() }));
        assert!(global.contains(&Scope::SessionScope { session_id: SessionId::new() }));
        assert!(global.contains(&Scope::ChannelScope { session_id: SessionId::new(), channel: "test".to_string() }));
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
        assert!(session1.contains(&Scope::ChannelScope { session_id: s1, channel: "c1".to_string() }));
        assert!(!session1.contains(&Scope::ChannelScope { session_id: s2, channel: "c1".to_string() }));
        assert!(!session1.contains(&Scope::GlobalScope));
    }

    #[test]
    fn test_channel_scope_contains() {
        let s1 = SessionId::new();
        let s2 = SessionId::new();
        let c1 = "chan1".to_string();
        let c2 = "chan2".to_string();
        let channel_scope = Scope::ChannelScope { session_id: s1, channel: c1.clone() };
        
        assert!(channel_scope.contains(&Scope::ChannelScope { session_id: s1, channel: c1 }));
        assert!(!channel_scope.contains(&Scope::ChannelScope { session_id: s1, channel: c2 }));
        assert!(!channel_scope.contains(&Scope::ChannelScope { session_id: s2, channel: "chan1".to_string() }));
        assert!(!channel_scope.contains(&Scope::SessionScope { session_id: s1 }));
        assert!(!channel_scope.contains(&Scope::GlobalScope));
    }
}