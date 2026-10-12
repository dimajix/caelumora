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
//! What lies in the scope is listed in the runtime's name, so that all of
//! it is found, whatever the caller may read; nothing of the list reaches
//! the caller. It is deleted and restored in the caller's name (issue 320):
//! the store asks each as it asks of anything deleted or restored alone,
//! and marks it as deleted by the caller. What may lie in a session's scope
//! is not always its owner's - an artifact another participant published
//! there, a contributor's agent -, so deleting deletes what the caller may
//! and fails with `SessionError::ScopeNotEmpty` if anything is left: the
//! holder is kept, and what is left is plain to see until its owners take
//! it elsewhere or delete it. Restoring restores what the caller may and
//! leaves the rest deleted, for its owners to restore: the holder comes
//! back all the same. Purging, the system's or an administrator's, is done
//! in the runtime's name.

use crate::agent::manager::AgentManager;
use crate::artifact::ArtifactManager;
use crate::group::GroupManager;
use crate::skill::SkillManager;
use crate::workspace::WorkspaceManager;
use caelumora_model::removal::Stored;
use caelumora_model::scope::{Scope, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::query::Query;
use caelumora_model::store::{EntityStore, EntityStores, StoreError, StoreResult};
use caelumora_model::value::name::ScopedName;
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};
use caelumora_model::agent::{Agent, Personality};
use caelumora_model::artifact::Artifact;
use caelumora_model::group::Group;
use caelumora_model::template::SessionTemplate;
use caelumora_model::skill::Skill;
use caelumora_model::workspace::Workspace;

/// Deletes, restores and purges what lies in a scope with what holds it
/// (issue 178), each kind by its owner where it has one (issue 319).
pub(crate) struct ScopedEntities {
    personalities: Arc<dyn EntityStore<Personality>>,
    session_templates: Arc<dyn EntityStore<SessionTemplate>>,
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
        stores: &EntityStores,
        agents: Arc<AgentManager>,
        workspaces: Arc<WorkspaceManager>,
        skills: Arc<SkillManager>,
        artifacts: Arc<ArtifactManager>,
        groups: Arc<GroupManager>,
    ) -> Self {
        Self { personalities: Arc::clone(&stores.personalities), session_templates: Arc::clone(&stores.session_templates), agents, workspaces, skills, artifacts, groups }
    }

    /// Deletes what lies in `holder` or within it on behalf of `ctx`, as
    /// far as they may delete it, and announces it as its owner announces a
    /// deletion. The groups of the participants, which the holder's access
    /// depends on while it is there, are deleted last, and only once nothing
    /// else is left.
    ///
    /// Called once the holder is marked deleted, so that what goes is
    /// deleted with it, not before it, and comes back with it
    /// (`restore_lying_in`); if something is left, the caller takes the
    /// mark away again.
    ///
    /// # Errors
    /// `SessionError::ScopeNotEmpty` if something is left that `ctx` may
    /// not delete - what they may is deleted all the same -, or whatever
    /// the store or an owner reports; what was deleted before stays
    /// deleted.
    pub(crate) async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        let mut personalities = 0;
        for personality in self.personalities.list(&system, &within(holder)).await?.items {
            personalities += usize::from(!deleted_unless_refused(self.personalities.delete(ctx, personality.id).await)?);
        }
        let mut templates = 0;
        for template in self.session_templates.list(&system, &within(holder)).await?.items {
            templates += usize::from(!deleted_unless_refused(self.session_templates.delete(ctx, template.id).await)?);
        }
        let mut left = vec![
            (Personality::KIND, personalities),
            (SessionTemplate::KIND, templates),
            (Agent::KIND, self.agents.delete_lying_in(ctx, holder).await?),
            (Workspace::KIND, self.workspaces.delete_lying_in(ctx, holder).await?),
            (Skill::KIND, self.skills.delete_lying_in(ctx, holder).await?),
            (Artifact::KIND, self.artifacts.delete_lying_in(ctx, holder).await?),
        ];
        left.retain(|(_, count)| *count > 0);
        if left.is_empty() {
            let groups = self.groups.delete_lying_in(ctx, holder).await?;
            if groups > 0 {
                left.push((Group::KIND, groups));
            }
        }
        if !left.is_empty() {
            return Err(crate::session::SessionError::ScopeNotEmpty { holder: holder.clone(), left }.into());
        }
        Ok(())
    }

    /// Restores what lies in `holder` or within it and was deleted with it -
    /// at `since`, when `holder` was deleted, or later - on behalf of `ctx`,
    /// and announces it as its owner announces a restoring. What was
    /// deleted before stays deleted, as a channel deleted before its
    /// session does; so does one whose name was taken meanwhile (issue 250)
    /// and one `ctx` may not restore (issue 320), and the others are
    /// restored all the same.
    ///
    /// # Errors
    /// Whatever else the store or an owner reports; what was restored
    /// before stays restored.
    pub(crate) async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> anyhow::Result<()> {
        let system = SecurityContext::system();
        for personality in deleted_since(self.personalities.list_deleted(&system, &within(holder)).await?.items, since) {
            restored_unless_refused(self.personalities.restore(ctx, personality.id).await, "personality", &personality.name)?;
        }
        for template in deleted_since(self.session_templates.list_deleted(&system, &within(holder)).await?.items, since) {
            restored_unless_refused(self.session_templates.restore(ctx, template.id).await, "session template", &template.name)?;
        }
        self.agents.restore_lying_in(ctx, holder, since).await?;
        self.workspaces.restore_lying_in(ctx, holder, since).await?;
        self.skills.restore_lying_in(ctx, holder, since).await?;
        self.artifacts.restore_lying_in(ctx, holder, since).await?;
        self.groups.restore_lying_in(ctx, holder, since).await?;
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
        for personality in self.personalities.list_deleted(&system, &within(holder)).await?.items {
            self.personalities.purge(&system, personality.id).await?;
        }
        for template in self.session_templates.list_deleted(&system, &within(holder)).await?.items {
            self.session_templates.purge(&system, template.id).await?;
        }
        self.agents.purge_lying_in(holder).await?;
        self.skills.purge_lying_in(holder).await?;
        self.groups.purge_lying_in(holder).await?;
        self.workspaces.purge_lying_in(holder).await?;
        self.artifacts.purge_lying_in(holder).await?;
        Ok(())
    }
}

/// Whether `deleted` says it was deleted; `false` also if it was not the
/// caller's to delete (issue 320), which leaves it.
pub(crate) fn deleted_unless_refused(deleted: StoreResult<bool>) -> StoreResult<bool> {
    match deleted {
        Err(StoreError::PermissionDenied) => Ok(false),
        deleted => deleted,
    }
}

/// What `restored` says was restored; `None` if it stays deleted: its name
/// was taken meanwhile, or it is not the caller's to restore (issue 320).
pub(crate) fn restored_unless_refused<T>(restored: StoreResult<T>, kind: &str, name: &ScopedName) -> StoreResult<Option<T>> {
    match restored {
        Ok(value) => Ok(Some(value)),
        Err(StoreError::NameTaken { .. }) => {
            warn!("The {} {} stays deleted: its name was taken while what holds its scope was deleted", kind, name);
            Ok(None)
        }
        Err(StoreError::NotFound { .. } | StoreError::PermissionDenied) => {
            info!("The {} {} stays deleted: it is not the caller's to restore", kind, name);
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
