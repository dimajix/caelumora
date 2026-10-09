//! The one writer of skills (issue 22): it makes, changes, deletes,
//! restores and purges them - stored first, then announced. Reading them is
//! the views' (`view::SkillView`); deleting and restoring them with what
//! holds their scope is `scoped.rs`'s, which marks them as it marks every
//! kind.
//!
//! A skill belongs to whoever it is made for - the principal of the context
//! it is made in, a user, or the system for the seed's - and lies in the
//! global scope, its owner's or a session's. Who may read, change and use
//! it is the policy's; that its owner is cleared for its classification is
//! asked here, as for a workspace, so that nobody writes what they could
//! not read.

use crate::event::EventBus;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{user_roles, Classification};
use caelumora_model::event::GlobalEvent;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::skill::{Skill, SkillId, SkillUpdateEvent};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResultExt};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::{Description, SkillBody};
use caelumora_model::value::ValueError;
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
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
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
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
    security: Arc<dyn SecurityManager>,
    /// Held by every write of a document, so that none is lost to another.
    writing: Mutex<()>,
}

impl fmt::Debug for SkillManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkillManager").finish_non_exhaustive()
    }
}

impl SkillManager {
    pub(crate) fn new(entities: Arc<dyn EntityStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>) -> Self {
        Self { entities, events, security, writing: Mutex::new(()) }
    }

    /// Whether `classification` may be given, and the one `ctx` acts for is
    /// cleared for it: what they write they can read.
    async fn require_classification(&self, ctx: &SecurityContext, classification: &Classification) -> SkillResult<()> {
        self.security.require(ctx, ActionType::Classify(classification), || SkillError::ClassificationRefused).await?;
        if ctx.principal_id == ActorId::System {
            return Ok(());
        }
        let roles = user_roles(self.entities.as_ref(), ctx.principal_id).await?;
        match self.security.may_see_channel(ctx, &roles, classification).await {
            true => Ok(()),
            false => Err(SkillError::NotCleared),
        }
    }

    /// Makes a skill of the one `ctx` acts for - a user, or the system - of
    /// `name` in `scope`, for what `description` says, telling `body`,
    /// classified `classification` (`CreateSkill`).
    ///
    /// # Errors
    /// `OwnerNotAUser`, `Invalid` for a name, description or body that does
    /// not fit, `ClassificationRefused`, `NotCleared`, or a `StoreError` -
    /// `NameTaken` among them, and `Validation` for a scope a skill does not
    /// lie in.
    pub async fn create(&self, ctx: &SecurityContext, name: &str, scope: Scope, description: &str, body: &str, classification: Classification) -> SkillResult<Skill> {
        let owner = match ctx.principal_id {
            owner @ (ActorId::User(_) | ActorId::System) => owner,
            _ => return Err(SkillError::OwnerNotAUser),
        };
        let skill = Skill::new(ScopedName::new(name, scope)?, Description::new(description)?, SkillBody::new(body)?, classification, owner);
        self.require_classification(ctx, &skill.classification).await?;

        let _writing = self.writing.lock().await;
        self.entities.create_skill(ctx, &skill).await?;
        self.events.publish(GlobalEvent::SkillCreated(skill.id), skill.security_attributes.clone());
        info!(skill = %skill.id, by = %ctx.caller_id, "Skill created");
        Ok(skill)
    }

    /// Renames skill `id` on behalf of `ctx` (`UpdateSkill`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rename(&self, ctx: &SecurityContext, id: SkillId, name: &str) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::NameChanged(name.to_string())).await
    }

    /// Says anew what skill `id` is for.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn describe(&self, ctx: &SecurityContext, id: SkillId, description: &str) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::DescriptionChanged(description.to_string())).await
    }

    /// Writes anew what skill `id` tells the model. A turn that loaded it
    /// keeps what it loaded.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn rewrite(&self, ctx: &SecurityContext, id: SkillId, body: &str) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::BodyChanged(body.to_string())).await
    }

    /// Reclassifies skill `id` as a whole; the one `ctx` acts for must be
    /// cleared for the new classification.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn reclassify(&self, ctx: &SecurityContext, id: SkillId, classification: Classification) -> SkillResult<Skill> {
        self.update(ctx, id, SkillUpdateEvent::ClassificationChanged(classification)).await
    }

    /// Applies `event` to skill `id` on behalf of `ctx`: stored first, which
    /// is where the permission is checked (`UpdateSkill`), then announced.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may read, `Invalid` if what
    /// `event` says does not fit, `ClassificationRefused` or `NotCleared`
    /// for a classification, or whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, id: SkillId, event: SkillUpdateEvent) -> SkillResult<Skill> {
        let _writing = self.writing.lock().await;
        let mut skill = self.entities.get_skill(ctx, id).await.map_err(store_error(id))?;
        match &event {
            SkillUpdateEvent::NameChanged(name) => skill.name = ScopedName::new(name, skill.name.scope.clone())?,
            SkillUpdateEvent::DescriptionChanged(description) => skill.description = Description::new(description)?,
            SkillUpdateEvent::BodyChanged(body) => skill.body = SkillBody::new(body)?,
            SkillUpdateEvent::ClassificationChanged(classification) => {
                self.require_classification(ctx, classification).await?;
                skill.classification = classification.clone();
            }
        }
        skill.updated_at = chrono::Utc::now();
        self.entities.update_skill(ctx, &skill).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::SkillUpdated(id, event), skill.security_attributes.clone());
        info!(skill = %id, by = %ctx.caller_id, "Skill changed");
        Ok(skill)
    }

    /// Deletes skill `id` (`DeleteSkill`): marked, kept. `false` if there
    /// was none the caller could see.
    ///
    /// # Errors
    /// Whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(skill) = self.entities.get_skill(ctx, id).await.optional()? else { return Ok(false) };
        if !self.entities.delete_skill(ctx, id).await? {
            return Ok(false);
        }
        self.events.publish(GlobalEvent::SkillDeleted(id), skill.security_attributes);
        info!(skill = %id, by = %ctx.caller_id, "Skill deleted");
        Ok(true)
    }

    /// Takes the deletion of skill `id` back (`Restore`).
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore, or
    /// whatever else the store reports - `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<()> {
        let _writing = self.writing.lock().await;
        self.entities.restore_skill(ctx, id).await.map_err(store_error(id))?;
        let skill = self.entities.get_skill(ctx, id).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::SkillRestored(id), skill.security_attributes);
        info!(skill = %id, by = %ctx.caller_id, "Skill restored");
        Ok(())
    }

    /// Removes the deleted skill `id` for good (`Purge`). Announces nothing,
    /// as no purge does. `false` if there was nothing deleted the caller
    /// could see.
    ///
    /// # Errors
    /// `PermissionDenied`, or whatever the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: SkillId) -> SkillResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(skill) = self.entities.get_deleted_skill(ctx, id).await.optional()? else { return Ok(false) };
        self.security.require(ctx, ActionType::Purge(EntityKind::Skill, &skill.security_attributes), || StoreError::PermissionDenied).await?;
        let purged = self.entities.purge_skill(ctx, id).await?;
        info!(skill = %id, by = %ctx.caller_id, "Skill purged");
        Ok(purged)
    }
}
