//! What lies in a scope goes with what holds it (issue 178): the
//! personalities, session templates, agents, workspaces, skills, artifacts
//! and groups - the participants of the session and its channels, issue
//! 286 - of a session's scope - and of the scopes of its channels - are
//! deleted with the session, restored with it and purged with it, a
//! workspace with its place (issue 251), an artifact with its content
//! (issue 271); those of a channel's scope are deleted and restored with the
//! channel. `SessionManager` and `SessionLoop` are the callers, through
//! `ScopedEntities`.
//!
//! What has an owner of its own - agents, workspaces, skills, artifacts and
//! groups - is deleted, restored and purged by it (`…_lying_in`, issue
//! 319): it announces what it deletes and restores as it announces its own
//! deletions, and holds its lock of writing as it does. Personalities and
//! session templates, which have none, are marked in the store here.
//!
//! All of it is done in the runtime's name: deleting, restoring or purging
//! what holds the scope was permitted, and what lies in it may be another's
//! to delete by the policy - a contributor's agent in the owner's session.

use crate::agent::manager::AgentManager;
use crate::artifact::ArtifactManager;
use crate::group::GroupManager;
use crate::skill::SkillManager;
use crate::workspace::WorkspaceManager;
use caelumora_model::removal::Stored;
use caelumora_model::scope::{Scope, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::query::Query;
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use caelumora_model::value::name::ScopedName;
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tracing::warn;

/// Deletes, restores and purges what lies in a scope with what holds it
/// (issue 178), each kind by its owner where it has one (issue 319).
pub(crate) struct ScopedEntities {
    entities: Arc<dyn EntityStore>,
    agents: Arc<AgentManager>,
    workspaces: Arc<WorkspaceManager>,
    skills: Arc<SkillManager>,
    artifacts: Arc<ArtifactManager>,
    groups: Arc<GroupManager>,
}

impl fmt::Debug for ScopedEntities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedEntities").finish_non_exhaustive()
    }
}

impl ScopedEntities {
    pub(crate) fn new(
        entities: Arc<dyn EntityStore>,
        agents: Arc<AgentManager>,
        workspaces: Arc<WorkspaceManager>,
        skills: Arc<SkillManager>,
        artifacts: Arc<ArtifactManager>,
        groups: Arc<GroupManager>,
    ) -> Self {
        Self { entities, agents, workspaces, skills, artifacts, groups }
    }

    /// Deletes what lies in `holder` or within it, and announces it as its
    /// owner announces a deletion.
    ///
    /// # Errors
    /// Whatever the store or an owner reports; what was deleted before
    /// stays deleted.
    pub(crate) async fn delete_lying_in(&self, holder: &Scope) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for personality in self.entities.list_personalities(&system, &within(holder)).await?.items {
            self.entities.delete_personality(&system, personality.id).await?;
        }
        for template in self.entities.list_session_templates(&system, &within(holder)).await?.items {
            self.entities.delete_session_template(&system, template.id).await?;
        }
        self.agents.delete_lying_in(holder).await?;
        self.workspaces.delete_lying_in(holder).await?;
        self.skills.delete_lying_in(holder).await?;
        self.artifacts.delete_lying_in(holder).await?;
        self.groups.delete_lying_in(holder).await?;
        Ok(())
    }

    /// Restores what lies in `holder` or within it and was deleted with it -
    /// at `since`, when `holder` was deleted, or later -, and announces it
    /// as its owner announces a restoring. What was deleted before stays
    /// deleted, as a channel deleted before its session does; so does one
    /// whose name was taken meanwhile (issue 250), and the others are
    /// restored all the same.
    ///
    /// # Errors
    /// Whatever else the store or an owner reports; what was restored
    /// before stays restored.
    pub(crate) async fn restore_lying_in(&self, holder: &Scope, since: DateTime<Utc>) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for personality in deleted_since(self.entities.list_deleted_personalities(&system, &within(holder)).await?.items, since) {
            unless_taken(self.entities.restore_personality(&system, personality.id).await, "personality", &personality.name)?;
        }
        for template in deleted_since(self.entities.list_deleted_session_templates(&system, &within(holder)).await?.items, since) {
            unless_taken(self.entities.restore_session_template(&system, template.id).await, "session template", &template.name)?;
        }
        self.agents.restore_lying_in(holder, since).await?;
        self.workspaces.restore_lying_in(holder, since).await?;
        self.skills.restore_lying_in(holder, since).await?;
        self.artifacts.restore_lying_in(holder, since).await?;
        self.groups.restore_lying_in(holder, since).await?;
        Ok(())
    }

    /// Purges what lies in `holder` or within it and is deleted, with what
    /// its owner purges with it: the agents' state, the workspaces' places,
    /// the artifacts' content.
    ///
    /// # Errors
    /// Whatever the store or an owner reports; what was purged before stays
    /// purged.
    pub(crate) async fn purge_lying_in(&self, holder: &Scope) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for personality in self.entities.list_deleted_personalities(&system, &within(holder)).await?.items {
            self.entities.purge_personality(&system, personality.id).await?;
        }
        for template in self.entities.list_deleted_session_templates(&system, &within(holder)).await?.items {
            self.entities.purge_session_template(&system, template.id).await?;
        }
        self.agents.purge_lying_in(holder).await?;
        self.skills.purge_lying_in(holder).await?;
        self.groups.purge_lying_in(holder).await?;
        self.workspaces.purge_lying_in(holder).await?;
        self.artifacts.purge_lying_in(holder).await?;
        Ok(())
    }
}

/// What `restored` says was restored; `None` if its name was taken
/// meanwhile, which leaves it deleted.
pub(crate) fn unless_taken<T>(restored: StoreResult<T>, kind: &str, name: &ScopedName) -> StoreResult<Option<T>> {
    match restored {
        Ok(value) => Ok(Some(value)),
        Err(StoreError::NameTaken { .. }) => {
            warn!("The {} {} stays deleted: its name was taken while what holds its scope was deleted", kind, name);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The query of what lies in `holder` or within it (issue 293), which the
/// store answers by the scope.
pub(crate) fn within<F: ScopedField>(holder: &Scope) -> Query<F> {
    Query { filter: holder.within(), ..Query::all() }
}

/// Those of `deleted` that were deleted at `since` or later - a comparison
/// a query does not make yet.
pub(crate) fn deleted_since<D: Stored>(deleted: Vec<D>, since: DateTime<Utc>) -> Vec<D> {
    deleted.into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
