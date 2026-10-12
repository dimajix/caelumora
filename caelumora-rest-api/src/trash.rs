//! What was deleted and lies in the trash (issue 164).

use caelumora_model::snapshot;
use caelumora_model::removal::Removal;
use serde::{Deserialize, Serialize};

/// Whether a listing lists what is there or what was deleted - the trash
/// (issue 164).
#[derive(Debug, Default, Deserialize)]
pub struct ListOptions {
    #[serde(default)]
    pub deleted: bool,
}

/// Something deleted - a session, a channel - in the trash (issue 164): who
/// deleted it and when, and whether the requesting user may restore it or
/// remove it for good.
#[derive(Debug, Serialize)]
pub struct DeletedResponse<I> {
    pub id: I,
    pub name: String,
    pub deleted: Removal,
    pub can_restore: bool,
    pub can_purge: bool,
}

impl<I> From<snapshot::DeletedSnapshot<I>> for DeletedResponse<I> {
    fn from(view: snapshot::DeletedSnapshot<I>) -> Self {
        Self { id: view.id, name: view.name, deleted: view.deleted, can_restore: view.can_restore, can_purge: view.can_purge }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::session::SessionId;
    
    use caelumora_model::user::UserId;
    use caelumora_model::snapshot;
    use caelumora_model::removal::Removal;

    #[test]
    fn something_deleted_is_answered_with_who_deleted_it_and_what_may_be_done() {
        let alice = ActorId::User(UserId::new());
        let id = SessionId::new();
        let removal = Removal::now(alice);
        let deleted = snapshot::DeletedSnapshot { id, name: "hers".to_string(), deleted: removal, can_restore: true, can_purge: false };
        let json = serde_json::to_value(DeletedResponse::from(deleted)).unwrap();
        assert_eq!(json["id"], serde_json::to_value(id).unwrap());
        assert_eq!(json["deleted"]["by"], serde_json::to_value(alice).unwrap());
        assert!(json["deleted"]["at"].is_string());
        assert_eq!((json["can_restore"].as_bool(), json["can_purge"].as_bool()), (Some(true), Some(false)));
    }

}
