//! What lies in a scope goes with what holds it (issue 178): the
//! personalities, session templates, agents, workspaces, skills and
//! artifacts of a session's scope - and of the scopes of its channels - are
//! deleted with the session, restored with it and purged with it, a
//! workspace with its place (issue 251), an artifact with its content
//! (issue 271); those of a channel's scope are deleted and restored with the
//! channel. `SessionManager` and `SessionLoop` are the callers. Deleting and
//! restoring mark what is stored, for every kind alike; purging a workspace
//! or an artifact removes its place or content first, which is its
//! manager's.
//!
//! All of it is done in the runtime's name: deleting, restoring or purging
//! what holds the scope was permitted, and what lies in it may be another's
//! to delete by the policy - a contributor's agent in the owner's session.

use caelumora_model::store::query::Query;
use crate::event::EventBus;
use crate::artifact::ArtifactManager;
use crate::workspace::WorkspaceManager;
use caelumora_model::event::GlobalEvent;
use caelumora_model::removal::Stored;
use caelumora_model::scope::{Scope, ScopedField};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StateStore, StoreError, StoreResult};
use caelumora_model::value::name::ScopedName;
use chrono::{DateTime, Utc};
use tracing::warn;

/// Deletes what lies in `holder` or within it, and announces the agents
/// among it as `SessionManager::delete_agent` does. They leave no channel:
/// they take part only within `holder` (`Scope::admits`).
///
/// # Errors
/// Whatever the store reports; what was deleted before stays deleted.
pub(crate) async fn delete_scoped_in(store: &dyn EntityStore, events: &EventBus, holder: &Scope) -> StoreResult<()> {
    let system = SecurityContext::system();
    for personality in store.list_personalities(&system, &within(holder)).await?.items {
        store.delete_personality(&system, personality.id).await?;
    }
    for template in store.list_session_templates(&system, &within(holder)).await?.items {
        store.delete_session_template(&system, template.id).await?;
    }
    for agent in store.list_agents(&system, &within(holder)).await?.items {
        store.delete_agent(&system, agent.id).await?;
        events.publish(GlobalEvent::AgentDeleted(agent.id), agent.security_attributes.clone());
    }
    for workspace in store.list_workspaces(&system, &within(holder)).await?.items {
        store.delete_workspace(&system, workspace.id).await?;
        events.publish(GlobalEvent::WorkspaceDeleted(workspace.id), workspace.security_attributes.clone());
    }
    for skill in store.list_skills(&system, &within(holder)).await?.items {
        store.delete_skill(&system, skill.id).await?;
        events.publish(GlobalEvent::SkillDeleted(skill.id), skill.security_attributes.clone());
    }
    for artifact in store.list_artifacts(&system, &within(holder)).await?.items {
        store.delete_artifact(&system, artifact.id).await?;
        events.publish(GlobalEvent::ArtifactDeleted(artifact.id), artifact.security_attributes.clone());
    }
    Ok(())
}

/// Restores what lies in `holder` or within it and was deleted with it -
/// at `since`, when `holder` was deleted, or later. What was deleted
/// before stays deleted, as a channel deleted before its session does; so
/// does one whose name was taken meanwhile (issue 250), and the others are
/// restored all the same.
///
/// # Errors
/// Whatever else the store reports; what was restored before stays
/// restored.
pub(crate) async fn restore_scoped_in(store: &dyn EntityStore, events: &EventBus, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
    let system = SecurityContext::system();
    for personality in deleted_since(store.list_deleted_personalities(&system, &within(holder)).await?.items, since) {
        unless_taken(store.restore_personality(&system, personality.id).await, "personality", &personality.name)?;
    }
    for template in deleted_since(store.list_deleted_session_templates(&system, &within(holder)).await?.items, since) {
        unless_taken(store.restore_session_template(&system, template.id).await, "session template", &template.name)?;
    }
    for agent in deleted_since(store.list_deleted_agents(&system, &within(holder)).await?.items, since) {
        if unless_taken(store.restore_agent(&system, agent.id).await, "agent", &agent.name)? {
            events.publish(GlobalEvent::AgentRestored(agent.id), agent.security_attributes.clone());
        }
    }
    for workspace in deleted_since(store.list_deleted_workspaces(&system, &within(holder)).await?.items, since) {
        if unless_taken(store.restore_workspace(&system, workspace.id).await, "workspace", &workspace.name)? {
            events.publish(GlobalEvent::WorkspaceRestored(workspace.id), workspace.security_attributes.clone());
        }
    }
    for skill in deleted_since(store.list_deleted_skills(&system, &within(holder)).await?.items, since) {
        if unless_taken(store.restore_skill(&system, skill.id).await, "skill", &skill.name)? {
            events.publish(GlobalEvent::SkillRestored(skill.id), skill.security_attributes.clone());
        }
    }
    for artifact in deleted_since(store.list_deleted_artifacts(&system, &within(holder)).await?.items, since) {
        if unless_taken(store.restore_artifact(&system, artifact.id).await, "artifact", &artifact.name)? {
            events.publish(GlobalEvent::ArtifactRestored(artifact.id), artifact.security_attributes.clone());
        }
    }
    Ok(())
}

/// Whether what `restored` says was restored; `false` if its name was
/// taken meanwhile, which leaves it deleted.
fn unless_taken(restored: StoreResult<()>, kind: &str, name: &ScopedName) -> StoreResult<bool> {
    match restored {
        Ok(()) => Ok(true),
        Err(StoreError::NameTaken { .. }) => {
            warn!("The {} {} stays deleted: its name was taken while what holds its scope was deleted", kind, name);
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Purges what lies in `holder` or within it and is deleted, and the
/// agents' state with them, as `SessionManager::purge_agent` does, the
/// workspaces' places and the artifacts' content.
///
/// # Errors
/// Whatever the stores or the storages of workspaces and artifacts
/// report.
pub(crate) async fn purge_scoped_in(store: &dyn EntityStore, states: &dyn StateStore, workspaces: &WorkspaceManager, artifacts: &ArtifactManager, holder: &Scope) -> anyhow::Result<()> {
    let system = SecurityContext::system();
    for personality in store.list_deleted_personalities(&system, &within(holder)).await?.items {
        store.purge_personality(&system, personality.id).await?;
    }
    for template in store.list_deleted_session_templates(&system, &within(holder)).await?.items {
        store.purge_session_template(&system, template.id).await?;
    }
    for agent in store.list_deleted_agents(&system, &within(holder)).await?.items {
        store.purge_agent(&system, agent.id).await?;
        states.delete_agent_state(agent.id).await?;
    }
    for skill in store.list_deleted_skills(&system, &within(holder)).await?.items {
        store.purge_skill(&system, skill.id).await?;
    }
    workspaces.purge_lying_in(holder).await?;
    artifacts.purge_lying_in(holder).await?;
    Ok(())
}

/// The query of what lies in `holder` or within it (issue 293), which the
/// store answers by the scope.
pub(crate) fn within<F: ScopedField>(holder: &Scope) -> Query<F> {
    Query { filter: holder.within(), ..Query::all() }
}

/// Those of `deleted` that were deleted at `since` or later - a comparison
/// a query does not make yet.
fn deleted_since<D: Stored>(deleted: Vec<D>, since: DateTime<Utc>) -> Vec<D> {
    deleted.into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
