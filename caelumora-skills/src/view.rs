//! Skills as a viewer reaches them (issue 22): among the skills they may read - the plugin's views,
//! an extension of the kernel's (`SkillViews`, issue 323). What an agent's work loads and writes is
//! the `SkillReader`'s and the `SkillWriter`'s (`extension`, issue 516), which the views serve. A
//! node decides nothing: whoever changes or loads it asks again.

use crate::access::SkillAccess;
use crate::extension::{SkillError, SkillReader, SkillResult, SkillWriter};
use crate::manager::SkillManager;
use crate::skill::{Skill, SkillBody, SkillId, SkillSnapshot};
use async_trait::async_trait;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::scope::{found_from, Scope};
use caelumora_domain::snapshot::DeletedSnapshot;
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::value::name::Name;
use caelumora_kernel::view::Views;
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

/// The reader of the views of a piece of work, in their viewer's name: what the plugin binds.
#[async_trait]
impl SkillReader for Views {
    async fn usable_skills(&self) -> SkillResult<Vec<Skill>> {
        let Some(place) = self.viewer().place() else { return Ok(Vec::new()) };
        let context = self.context();
        let here = Scope::ChannelScope { session_id: place.session.id, channel_id: place.channel };
        let owner = context.principal_id.as_user_id();
        // Those seen from here, which the store is asked for alone (issue 293).
        let seen = Query::filtered(Filter::seen_from(&here, owner));
        let skills = self.skill_manager()?.store().list(context, &seen).await?.items;
        let mut usable = Vec::with_capacity(skills.len());
        for skill in skills {
            if self.access_policy().may_use_skill(context, &skill.security_attributes).await
                && self.confidentiality().may_publish(
                    context,
                    &skill.classification,
                    &place.classification,
                )
            {
                usable.push(skill);
            }
        }
        Ok(found_from(&usable, &here, owner).into_iter().cloned().collect())
    }

    async fn usable_skill_named(&self, name: &str) -> SkillResult<Skill> {
        let not_found = || SkillError::NoSkillNamed(name.to_string());
        let wanted = Name::new(name).map_err(|_| not_found())?;
        SkillReader::usable_skills(self)
            .await?
            .into_iter()
            .find(|skill| skill.name.name == wanted)
            .ok_or_else(not_found)
    }
}

/// The writer of the views of a piece of work, in their viewer's name: what the plugin binds.
#[async_trait]
impl SkillWriter for Views {
    fn owner_scope(&self) -> Option<Scope> {
        let user_id = self.context().principal_id.as_user_id()?;
        Some(Scope::UserScope { user_id })
    }

    async fn create(
        &self,
        name: &str,
        scope: Scope,
        description: &str,
        body: &str,
        classification: Classification,
    ) -> SkillResult<Skill> {
        let manager = self.skill_manager()?;
        manager.create(self.context(), name, scope, description, body, classification).await
    }

    async fn reclassify(&self, id: SkillId, classification: Classification) -> SkillResult<Skill> {
        self.skill_manager()?.reclassify(self.context(), id, classification).await
    }

    async fn describe(&self, id: SkillId, description: &str) -> SkillResult<Skill> {
        self.skill_manager()?.describe(self.context(), id, description).await
    }

    async fn rewrite(&self, id: SkillId, body: &str) -> SkillResult<Skill> {
        self.skill_manager()?.rewrite(self.context(), id, body).await
    }
}
