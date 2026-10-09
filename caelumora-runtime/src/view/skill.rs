//! A skill as a viewer reaches it (issue 22): a user among the skills they
//! may read, an agent among those it may load where it is. A node decides
//! nothing: whoever changes or loads it asks again.

use super::Views;
use caelumora_model::confidentiality::Classification;
use caelumora_model::scope::{Scope, Scoped};
use caelumora_model::security::AccessQuestions;
use caelumora_model::skill::{Skill, SkillId};
use caelumora_model::snapshot::SkillSnapshot;
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::{Description, SkillBody};

#[derive(Clone)]
pub struct SkillView {
    views: Views,
    skill: Skill,
}

impl SkillView {
    pub(super) fn of(views: Views, skill: Skill) -> Self {
        Self { views, skill }
    }

    pub fn id(&self) -> SkillId {
        self.skill.id
    }

    pub fn name(&self) -> &str {
        self.skill.name.name.as_str()
    }

    /// Where it lies: the global scope, its owner's or a session's.
    pub fn scope(&self) -> &Scope {
        &self.skill.name.scope
    }

    pub fn description(&self) -> &Description {
        &self.skill.description
    }

    /// What it tells the model: what loading it gives.
    pub fn body(&self) -> &SkillBody {
        &self.skill.body
    }

    pub fn classification(&self) -> &Classification {
        &self.skill.classification
    }

    /// Whether the viewer may change it and delete it (`UpdateSkill`): its
    /// owner.
    pub async fn may_change(&self) -> bool {
        self.views.security().may_change_skill(self.views.context(), &self.skill.security_attributes).await
    }

    pub async fn snapshot(&self) -> SkillSnapshot {
        SkillSnapshot {
            id: self.skill.id,
            name: self.name().to_string(),
            scope: self.skill.name.scope.clone(),
            description: self.skill.description.clone(),
            classification: self.skill.classification.clone(),
            owner: self.skill.security_attributes.owner_id,
            can_change: self.may_change().await,
        }
    }
}

impl Scoped for SkillView {
    fn scoped_name(&self) -> ScopedName {
        self.skill.name.clone()
    }
}
