//! The wire of skills (issue 22): as whoever may read them sees them, and
//! how they are written.

use crate::skill::{SkillBody, SkillId, SkillSnapshot};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::session::SessionId;
use caelumora_model::value::text::Description;
use caelumora_rest_api::scope::ScopeKind;
use serde::{Deserialize, Serialize};

/// A skill (issue 22) as whoever may read it sees it; with its `body` where
/// it is asked for alone, not where skills are listed.
#[derive(Debug, Serialize)]
pub struct SkillResponse {
    pub id: SkillId,
    pub name: String,
    /// Where it lies: `session` - written by an agent for one session,
    /// which `session_id` names -, `user` or `global`.
    pub scope: ScopeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub description: Description,
    pub classification: Classification,
    /// Whose it is: a user, or the system for a skill of the seed.
    pub owner: ActorId,
    /// Whether you may change it - rename, describe, rewrite, reclassify -
    /// and delete it: its owner.
    pub can_change: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<SkillBody>,
}

impl From<SkillSnapshot> for SkillResponse {
    fn from(view: SkillSnapshot) -> Self {
        let (scope, session_id) = ScopeKind::of(&view.scope);
        Self { id: view.id, name: view.name, scope, session_id, description: view.description, classification: view.classification, owner: view.owner, can_change: view.can_change, body: None }
    }
}

/// A skill to make (issue 22): in the user's scope or the global one, and
/// classified `internal` without compartments unless named.
#[derive(Debug, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    #[serde(default)]
    pub scope: ScopeKind,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub classification: Option<Classification>,
}

#[derive(Debug, Deserialize)]
pub struct RenameSkillRequest {
    pub name: String,
}

/// What a skill is for, in a line, replacing what it said.
#[derive(Debug, Deserialize)]
pub struct SkillDescriptionRequest {
    pub description: String,
}

/// What a skill tells the model, replacing what it told.
#[derive(Debug, Deserialize)]
pub struct SkillBodyRequest {
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::scope::Scope;

    #[test]
    fn a_skill_is_answered_with_its_scope_and_its_body_only_alone() {
        let session_id = SessionId::new();
        let snapshot = SkillSnapshot {
            id: SkillId::new(), name: "deploy".to_string(), scope: Scope::SessionScope { session_id },
            description: Description::new("How we deploy").unwrap(), classification: Classification::CHANNEL_DEFAULT,
            owner: ActorId::System, can_change: false,
        };
        let listed = serde_json::to_value(SkillResponse::from(snapshot.clone())).unwrap();
        assert_eq!((listed["scope"].clone(), listed["session_id"].clone()), (serde_json::json!("session"), serde_json::to_value(session_id).unwrap()));
        assert!(listed.get("body").is_none(), "not where skills are listed");
        let alone = SkillResponse { body: Some(SkillBody::new("1. Build.").unwrap()), ..snapshot.into() };
        assert_eq!(serde_json::to_value(alone).unwrap()["body"], "1. Build.");
    }

}
