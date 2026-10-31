//! What an agent's work does with skills (issue 516): the skills it may load where it reads from,
//! and those of its owner it writes - the plugin's service an application reaches, bound by the
//! plugin to every reading (`SessionReader::extension`), so that it acts in the reader's name. The
//! component of skills names this trait, never the `SkillManager` or the views.

use crate::access::SkillAccess;
use crate::manager::{SkillError, SkillResult};
use crate::skill::{Skill, SkillId};
use crate::view::SkillViews;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::scope::{found_from, Scope};
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::value::name::Name;
use caelumora_runtime::view::Views;

/// The skills an agent's work reads and writes where it reads from, in the reader's name.
#[async_trait]
pub trait SkillCatalogue: Send + Sync {
    /// The skills the reader may load where it reads from (issue 22): of those the store lists
    /// for it - read and cleared for, as for a user (issues 304, 322) -, each it may use
    /// (`UseSkill`) and whose classification it may put into its channel (`may_publish`, as what
    /// a turn writes is asked) - and of those, for every name the one found from its channel (the
    /// innermost: the session's before its owner's before the global one). By name. None for a
    /// reader who reads from no place: skills are loaded into turns.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn usable_skills(&self) -> SkillResult<Vec<Skill>>;

    /// The skill of `name` the reader may load where it reads from (`usable_skills`).
    ///
    /// # Errors
    /// `NoSkillNamed` if there is none, and equally if it may not be used here; `Store` if the
    /// store cannot list them.
    async fn usable_skill_named(&self, name: &str) -> SkillResult<Skill>;

    /// The scope of the reader's owner, where a skill of theirs lies; `None` for an owner who is
    /// no user.
    fn owner_scope(&self) -> Option<Scope>;

    /// Writes the skill `name` of the reader's owner in `scope`, for what `description` says,
    /// telling `body`, classified `classification` (`SkillManager::create`).
    ///
    /// # Errors
    /// As `SkillManager::create`.
    async fn create(
        &self,
        name: &str,
        scope: Scope,
        description: &str,
        body: &str,
        classification: Classification,
    ) -> SkillResult<Skill>;

    /// Reclassifies skill `id` as a whole (`SkillManager::reclassify`).
    ///
    /// # Errors
    /// As `SkillManager::reclassify`.
    async fn reclassify(&self, id: SkillId, classification: Classification) -> SkillResult<Skill>;

    /// Says anew what skill `id` is for (`SkillManager::describe`).
    ///
    /// # Errors
    /// As `SkillManager::describe`.
    async fn describe(&self, id: SkillId, description: &str) -> SkillResult<Skill>;

    /// Writes anew what skill `id` tells the model (`SkillManager::rewrite`).
    ///
    /// # Errors
    /// As `SkillManager::rewrite`.
    async fn rewrite(&self, id: SkillId, body: &str) -> SkillResult<Skill>;
}

/// The catalogue of the views a reading stands on, in their viewer's name: what the plugin binds.
#[async_trait]
impl SkillCatalogue for Views {
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
        SkillCatalogue::usable_skills(self)
            .await?
            .into_iter()
            .find(|skill| skill.name.name == wanted)
            .ok_or_else(not_found)
    }

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
