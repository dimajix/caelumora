//! The part of the kernel's API that is skills' (issue 516): what an agent's work does with them,
//! and what that fails with. The work loads the skills it may where it reads from (`SkillReader`),
//! and writes those of its owner (`SkillWriter`) - the plugin's service a library reaches, its two
//! faces bound by the plugin to the views of every piece of work, so that they act in the work's
//! context: the reader handed out where the work reads (`Situation::reader`), the writer where it
//! acts (`Turn::writer`). The component of skills names these traits, never the `SkillManager` or
//! the views. Their error, `SkillError`, is the one error of skills, which the manager, the views
//! and the routes answer in as well.

use crate::skill::{Skill, SkillId};
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::scope::Scope;
use caelumora_model::store::StoreError;
use caelumora_model::value::ValueError;

/// The skills an agent's work may load where it reads from.
#[async_trait]
pub trait SkillReader: Send + Sync {
    /// The skills the work may load where it reads from (issue 22): of those the store lists
    /// for it - read and cleared for, as for a user (issues 304, 322) -, each it may use
    /// (`UseSkill`) and whose classification it may put into its channel (`may_publish`, as what
    /// a turn writes is asked) - and of those, for every name the one found from its channel (the
    /// innermost: the session's before its owner's before the global one). By name. None for
    /// work that reads from no place: skills are loaded into turns.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn usable_skills(&self) -> SkillResult<Vec<Skill>>;

    /// The skill of `name` the work may load where it reads from (`usable_skills`).
    ///
    /// # Errors
    /// `NoSkillNamed` if there is none, and equally if it may not be used here; `Store` if the
    /// store cannot list them.
    async fn usable_skill_named(&self, name: &str) -> SkillResult<Skill>;
}

/// The skills of its owner an agent's work writes.
#[async_trait]
pub trait SkillWriter: Send + Sync {
    /// The scope of the work's owner, where a skill of theirs lies; `None` for an owner who is
    /// no user.
    fn owner_scope(&self) -> Option<Scope>;

    /// Writes the skill `name` of the work's owner in `scope`, for what `description` says,
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

/// What a skill's operations fail with (issue 22).
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
    /// The kernel was built without the plugin (issue 323).
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
