//! Skills as a user reaches them (issue 22): among the skills they may read - the plugin's views,
//! an extension of the kernel's (`SkillViews`, issue 323). What an agent's work loads and writes is
//! the `SkillReader`'s and the `SkillWriter`'s (`extension`, issue 516). A node decides nothing:
//! whoever changes or loads it asks again.

use crate::access::SkillAccess;
use crate::manager::{SkillError, SkillManager, SkillResult};
use crate::skill::{Skill, SkillBody, SkillId, SkillSnapshot};
use async_trait::async_trait;
use caelumora_kernel::view::Views;
use caelumora_model::snapshot::DeletedSnapshot;
use caelumora_model::store::query::Query;
use caelumora_model::store::StoreResultExt;
use std::sync::Arc;

/// The skills the views reach (issue 323): what the kernel's `Views` offer of skills, from the
/// plugin.
#[async_trait]
pub trait SkillViews {
    /// The skills the store lists for the viewer (issues 22, 322): their
    /// own, those shared with them, the system's, that they are cleared
    /// for.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn skills(&self) -> SkillResult<Vec<SkillView>>;

    /// Skill `id`, if the store lets the viewer reach it - read it and be
    /// cleared for it.
    ///
    /// # Errors
    /// `NotFound` if there is none they may reach.
    async fn skill(&self, id: SkillId) -> SkillResult<SkillView>;

    /// The skills the viewer deleted and may read as such.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn deleted_skills(&self) -> SkillResult<Vec<DeletedSnapshot<SkillId>>>;

    /// The one writer of skills, which the plugin offers as a service.
    ///
    /// # Errors
    /// `NotJoined` if the kernel was built without the plugin.
    fn skill_manager(&self) -> SkillResult<&Arc<SkillManager>>;
}

#[async_trait]
impl SkillViews for Views {
    async fn skills(&self) -> SkillResult<Vec<SkillView>> {
        let skills = self.skill_manager()?.store().list(self.context(), &Query::all()).await?.items;
        Ok(skills.into_iter().map(|skill| SkillView::of(self.clone(), skill)).collect())
    }

    async fn skill(&self, id: SkillId) -> SkillResult<SkillView> {
        match self.skill_manager()?.store().get(self.context(), id).await.optional()? {
            Some(skill) => Ok(SkillView::of(self.clone(), skill)),
            None => Err(SkillError::NotFound(id)),
        }
    }

    async fn deleted_skills(&self) -> SkillResult<Vec<DeletedSnapshot<SkillId>>> {
        Ok(self
            .deleted_of(self.skill_manager()?.store().as_ref(), &Query::all(), |skill| {
                skill.name.to_string()
            })
            .await?)
    }

    fn skill_manager(&self) -> SkillResult<&Arc<SkillManager>> {
        self.service::<SkillManager>().ok_or(SkillError::NotJoined)
    }
}

#[derive(Clone)]
pub struct SkillView {
    views: Views,
    skill: Skill,
}

impl SkillView {
    fn of(views: Views, skill: Skill) -> Self {
        Self { views, skill }
    }

    pub fn name(&self) -> &str {
        self.skill.name.name.as_str()
    }

    /// What it tells the model: what loading it gives.
    pub fn body(&self) -> &SkillBody {
        &self.skill.body
    }

    /// Whether the viewer may change it and delete it (`UpdateSkill`): its
    /// owner.
    pub async fn may_change(&self) -> bool {
        self.views
            .access_policy()
            .may_change_skill(self.views.context(), &self.skill.security_attributes)
            .await
    }

    pub async fn snapshot(&self) -> SkillSnapshot {
        SkillSnapshot {
            id: self.skill.id,
            name: self.name().to_string(),
            scope: self.views.scope_snapshot(&self.skill.name.scope).await,
            description: self.skill.description.clone(),
            classification: self.skill.classification.clone(),
            owner: self.skill.security_attributes.owner_id,
            can_change: self.may_change().await,
        }
    }
}
