//! The one writer of skills (issue 22): it makes, changes, deletes,
//! restores and purges them - stored first, then announced. Reading them is
//! the views' (`view::SkillViews`). Those of a session's scope go with the
//! session (`scoped`), with the manager as their `ScopeMember`.
//!
//! A skill belongs to whoever it is made for - the principal of the context
//! it is made in, a user, or the system for the seed's - and lies in the
//! global scope, its owner's or a session's. Who may read, change and use
//! it is the policy's; that its owner is cleared for its classification is
//! asked here, as for a workspace, so that nobody writes what they could
//! not read. A skill named by its id is reached by the store, as the views
//! reach it: read and cleared for (issues 312, 322).

use crate::skill::{Skill, SkillBody, SkillId, SkillUpdateEvent};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{
    Classification, ConfidentialityPolicy, ConfidentialityQuestions,
};
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StoreError, StoreResultExt};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_runtime::event::EventBus;
use caelumora_runtime::scoped::{ScopeHolders, ScopeMember};
use chrono::Utc;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// No such skill, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no skill {0}")]
    NotFound(SkillId),
    #[error("a skill belongs to a user or to the system")]
    OwnerNotAUser,
    #[error("the classification may not be given")]
    ClassificationRefused,
    #[error("you are not cleared for the classification of the skill")]
    NotCleared,
    /// What an agent asks for by name: no skill of the name it may load
    /// where it is, alike whether there is one at all (issue 22).
    #[error("there is no skill named \"{0}\" that can be loaded here")]
    NoSkillNamed(String),
    /// The runtime was built without the plugin (issue 323).
    #[error("skills are not offered here")]
    NotJoined,
    #[error(transparent)]
    Invalid(#[from] ValueError),
    /// What the store reported, as its source: judged as a store error
    /// wherever errors are turned into messages.
    #[error("{0}")]
    Store(#[from] StoreError),
}

pub type SkillResult<T> = Result<T, SkillError>;

/// `error` of the store about skill `id`: not found is one case with not
/// readable.
fn store_error(id: SkillId) -> impl FnOnce(StoreError) -> SkillError {
    move |error| match error {
        StoreError::NotFound { .. } => SkillError::NotFound(id),
        error => SkillError::Store(error),
    }
}

pub struct SkillManager {
    skills: Arc<dyn EntityStore<Skill>>,
    /// Asked whether a skill may be put into a scope (issue 458).
    holders: Arc<ScopeHolders>,
    events: Arc<EventBus>,
    /// What may flow where, and who is cleared for what (issue 304).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// Held by every write of a document, so that none is lost to another.
    writing: Mutex<()>,
}

impl fmt::Debug for SkillManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillManager").finish_non_exhaustive()
    }
}

impl SkillManager {
    /// Who may do what is asked by `skills`, the store every write goes
    /// through - the clearance for a skill reached included (issue 322);
    /// the manager asks the flow alone (issue 304).
    pub fn new(
        skills: Arc<dyn EntityStore<Skill>>,
        holders: Arc<ScopeHolders>,
        events: Arc<EventBus>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self { skills, holders, events, confidentiality, writing: Mutex::new(()) }
    }

    /// The store of skills, which the views read (`SkillViews`).
    pub(crate) fn store(&self) -> &Arc<dyn EntityStore<Skill>> {
        &self.skills
    }

    /// Whether the one `ctx` acts for is cleared for `classification`: what
    /// they write they can read - the flow, the `ConfidentialityPolicy`'s
    /// (issue 304), asked before the store asks their permission: it tells
    /// them nothing of others (issue 312).
    fn require_flow(
        &self,
        ctx: &SecurityContext,
        classification: &Classification,
    ) -> SkillResult<()> {
        self.confidentiality.require_cleared_for(ctx, classification, || SkillError::NotCleared)
    }

    /// Makes a skill of the one `ctx` acts for - a user, or the system - of
    /// `name` in `scope`, for what `description` says, telling `body`,
    /// classified `classification` (`CreateSkill`, asked by the store).
    ///
    /// # Errors
    /// `OwnerNotAUser`, `Invalid` for a name, description or body that does
    /// not fit, `ClassificationRefused`, `NotCleared`, or a `StoreError` -
    /// `NameTaken` among them, and `Validation` for a scope a skill does not
    /// lie in.
    pub async fn create(
        &self,
        ctx: &SecurityContext,
        name: &str,
        scope: Scope,
        description: &str,
        body: &str,
        classification: Classification,
    ) -> SkillResult<Skill> {
        let owner = match ctx.principal_id {
            owner @ (ActorId::User(_) | ActorId::System) => owner,
            _ => return Err(SkillError::OwnerNotAUser),
        };
        let skill = Skill::new(
            ScopedName::new(name, scope)?,
            Description::new(description)?,
            SkillBody::new(body)?,
            classification,
            owner,
        );
        self.confidentiality
            .require_classifiable(&skill.classification, || SkillError::ClassificationRefused)?;
        self.require_flow(ctx, &skill.classification)?;
        // Asked before the store asks `CreateSkill`, which is the first act asked here: the
        // session is read in the caller's name, who learns nothing they could not read.
        self.holders.require(ctx, &skill.name.scope).await?;

        let _writing = self.writing.lock().await;
        self.skills.create(ctx, &skill).await?;
        self.holders.keep_made(self, &skill).await?;
        self.events.announce(GlobalEvent::created::<Skill>(&skill.id), &skill);
        info!(skill = %skill.id, by = %ctx.caller_id, "Skill created");
        Ok(skill)
    }

    /// Renames skill `id` on behalf of `ctx` (`UpdateSkill`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rename(
        &self,
        ctx: &SecurityContext,
        id: SkillId,
        name: &str,
    ) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::NameChanged(name.to_string())).await
    }

    /// Says anew what skill `id` is for.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn describe(
        &self,
        ctx: &SecurityContext,
        id: SkillId,
        description: &str,
    ) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::DescriptionChanged(description.to_string())).await
    }

    /// Writes anew what skill `id` tells the model. A turn that loaded it
    /// keeps what it loaded.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rewrite(
        &self,
        ctx: &SecurityContext,
        id: SkillId,
        body: &str,
    ) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::BodyChanged(body.to_string())).await
    }

    /// Reclassifies skill `id` as a whole; the one `ctx` acts for must be
    /// cleared for the new classification.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn reclassify(
        &self,
        ctx: &SecurityContext,
        id: SkillId,
        classification: Classification,
    ) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::ClassificationChanged(classification)).await
    }

    /// Applies `event` to skill `id` on behalf of `ctx`: reached by the
    /// store, stored, which is where the permission is checked
    /// (`UpdateSkill`), then announced. A new classification is validated
    /// and its flow asked before.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read or is cleared for,
    /// `Invalid` if what `event` says does not fit, `ClassificationRefused`
    /// or `NotCleared` for a classification, or whatever the store reports.
    async fn update(
        &self,
        ctx: &SecurityContext,
        id: SkillId,
        event: SkillUpdateEvent,
    ) -> SkillResult<Skill> {
        let _writing = self.writing.lock().await;
        let mut skill = self.skills.get(ctx, id).await.map_err(store_error(id))?;
        // Announced to who reaches it as it was, too: who no longer does
        // learns so (issue 312).
        let had = skill.classification.clone();
        match &event {
            SkillUpdateEvent::NameChanged(name) => {
                skill.name = ScopedName::new(name, skill.name.scope.clone())?
            }
            SkillUpdateEvent::DescriptionChanged(description) => {
                skill.description = Description::new(description)?
            }
            SkillUpdateEvent::BodyChanged(body) => skill.body = SkillBody::new(body)?,
            SkillUpdateEvent::ClassificationChanged(classification) => {
                self.confidentiality
                    .require_classifiable(classification, || SkillError::ClassificationRefused)?;
                self.require_flow(ctx, classification)?;
                skill.classification = classification.clone();
            }
        }
        skill.updated_at = Utc::now();
        self.skills.update(ctx, &skill).await.map_err(store_error(id))?;
        self.events.publish_classified(
            GlobalEvent::updated::<Skill>(&id, &event),
            skill.security_attributes.clone(),
            &[had, skill.classification.clone()],
        );
        info!(skill = %id, by = %ctx.caller_id, "Skill changed");
        Ok(skill)
    }

    /// Deletes skill `id` (`DeleteSkill`): marked, kept. `false` if there
    /// was none the caller could reach, by the store.
    ///
    /// # Errors
    /// Whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(skill) = self.skills.get(ctx, id).await.optional()? else { return Ok(false) };
        if !self.skills.delete(ctx, id, &Removal::now(ctx.caller_id)).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<Skill>(&id), &skill);
        info!(skill = %id, by = %ctx.caller_id, "Skill deleted");
        Ok(true)
    }

    /// Takes the deletion of skill `id` back (`Restore`), if what holds its
    /// scope is still there (issue 458). Returns the skill as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore, or if
    /// what holds its scope is gone, or whatever else the store reports -
    /// `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<Skill> {
        let _writing = self.writing.lock().await;
        let skill = self.holders.restore(self, ctx, id).await.map_err(store_error(id))?;
        self.events.announce(GlobalEvent::restored::<Skill>(&id), &skill);
        info!(skill = %id, by = %ctx.caller_id, "Skill restored");
        Ok(skill)
    }

    /// Removes the deleted skill `id` for good (`Purge`, asked by the store:
    /// nothing is done before, issue 304). Announces nothing, as no purge
    /// does. `false` if there was nothing deleted the caller could see.
    ///
    /// # Errors
    /// `PermissionDenied`, or whatever the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<bool> {
        let _writing = self.writing.lock().await;
        let purged = self.skills.purge(ctx, id).await?;
        info!(skill = %id, by = %ctx.caller_id, "Skill purged");
        Ok(purged)
    }
}

/// Skills go with what holds the scope they lie in, announced as `delete` and `restore` announce
/// them, under the lock of writing (`scoped`).
impl ScopeMember for SkillManager {
    type Entity = Skill;

    fn store(&self) -> &dyn EntityStore<Skill> {
        self.skills.as_ref()
    }

    fn events(&self) -> Option<&dyn EventPublisher> {
        Some(self.events.as_ref())
    }

    fn writing(&self) -> Option<&Mutex<()>> {
        Some(&self.writing)
    }
}
