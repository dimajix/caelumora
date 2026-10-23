//! Skills as a viewer reaches them (issue 22): a user among the skills they
//! may read, an agent among those it may load where it is - the plugin's
//! views, an extension of the runtime's (`SkillViews`, issue 323). A node
//! decides nothing: whoever changes or loads it asks again.

use crate::access::SkillAccess;
use crate::manager::{SkillError, SkillManager, SkillResult};
use crate::skill::{self, Skill, SkillBody, SkillId, SkillSnapshot};
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::scope::{found_from, Scope, Scoped};
use caelumora_model::security::AccessQuestions;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::DeletedSnapshot;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{StoreResultExt, Stored};
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::value::text::Description;
use caelumora_runtime::view::Views;
use std::sync::Arc;

/// The skills the views reach (issue 323): what the runtime's `Views`
/// offer of skills, from the plugin.
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

    /// The skills the viewer deleted and may read as such: the trash of
    /// skills.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn deleted_skills(&self) -> SkillResult<Vec<DeletedSnapshot<SkillId>>>;

    /// The skills an agent may load where it reads from (issue 22): of those
    /// the store lists for it - read and cleared for, as for a user (issues
    /// 304, 322) -, each it may use (`UseSkill`) and whose classification it
    /// may put into its channel (`may_publish`, as what a turn writes is
    /// asked) - and of those, for every name the one found from its channel
    /// (the innermost: the session's before its owner's before the global
    /// one). By name. None for a viewer who reads from no place: skills are
    /// loaded into turns.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn usable_skills(&self) -> SkillResult<Vec<SkillView>>;

    /// The skill of `name` the agent may load where it reads from
    /// (`usable_skills`).
    ///
    /// # Errors
    /// `NoSkillNamed` if there is none, and equally if it may not be used
    /// here; `Store` if the store cannot list them.
    async fn usable_skill_named(&self, name: &str) -> SkillResult<SkillView>;

    /// The one writer of skills, which the plugin offers as a service.
    ///
    /// # Errors
    /// `NotJoined` if the runtime was built without the plugin.
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
        let skills = self.skill_manager()?.store().list_deleted(self.context(), &Query::all()).await?.items;
        let access_policy = self.access_policy();
        let mut deleted = Vec::with_capacity(skills.len());
        for skill in skills {
            let Some(removal) = skill.deleted.clone() else { continue };
            let attributes = &skill.security_attributes;
            deleted.push(DeletedSnapshot {
                id: skill.id,
                name: skill.name.to_string(),
                deleted: removal,
                can_restore: access_policy.may_restore(self.context(), Skill::KIND, attributes).await,
                can_purge: access_policy.may_purge(self.context(), Skill::KIND, attributes).await,
            });
        }
        Ok(deleted)
    }

    async fn usable_skills(&self) -> SkillResult<Vec<SkillView>> {
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
                && self.confidentiality().may_publish(context, &skill.classification, &place.classification)
            {
                usable.push(skill);
            }
        }
        Ok(found_from(&usable, &here, owner).into_iter().map(|skill| SkillView::of(self.clone(), skill.clone())).collect())
    }

    async fn usable_skill_named(&self, name: &str) -> SkillResult<SkillView> {
        let not_found = || SkillError::NoSkillNamed(name.to_string());
        let wanted = Name::new(name).map_err(|_| not_found())?;
        self.usable_skills().await?.into_iter().find(|skill| skill.scoped_name().name == wanted).ok_or_else(not_found)
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

    /// That it was loaded in `channel` of `session`, as it is now: knowledge
    /// of the turn it was loaded into (`KnowledgeSource::Entity`).
    pub fn knowledge(&self, channel: ChannelId, session: SessionId) -> KnowledgeRef {
        KnowledgeRef {
            channel,
            session,
            source: KnowledgeSource::entity::<Skill>(self.skill.id, Some(skill::digest(self.skill.body.as_str()))),
            classification: Some(self.skill.classification.clone()),
        }
    }

    /// Whether the viewer may change it and delete it (`UpdateSkill`): its
    /// owner.
    pub async fn may_change(&self) -> bool {
        self.views.access_policy().may_change_skill(self.views.context(), &self.skill.security_attributes).await
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
