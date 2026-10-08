//! What lies in a scope goes with what holds it (issue 178): the
//! personalities, session templates, agents and workspaces of a session's
//! scope - and of the scopes of its channels - are deleted with the
//! session, restored with it and purged with it, a workspace with its
//! place (issue 251); those of a channel's scope are deleted and restored
//! with the channel. `SessionManager` and `SessionLoop` are the callers.
//! Deleting and restoring mark what is stored, for every kind alike;
//! purging a workspace removes its place first, which is its manager's.
//!
//! All of it is done in the runtime's name: deleting, restoring or purging
//! what holds the scope was permitted, and what lies in it may be another's
//! to delete by the policy - a contributor's agent in the owner's session.

use crate::event::EventBus;
use crate::workspace::WorkspaceManager;
use caelumora_model::event::GlobalEvent;
use caelumora_model::removal::Stored;
use caelumora_model::scope::{Scoped, Scope};
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
    for personality in lying_in(store.list_personalities(&system).await?, holder) {
        store.delete_personality(&system, personality.id).await?;
    }
    for template in lying_in(store.list_session_templates(&system).await?, holder) {
        store.delete_session_template(&system, template.id).await?;
    }
    for agent in lying_in(store.list_agents(&system).await?, holder) {
        store.delete_agent(&system, agent.id).await?;
        events.publish(GlobalEvent::AgentDeleted(agent.id), agent.security_attributes.clone());
    }
    for workspace in lying_in(store.list_workspaces(&system).await?, holder) {
        store.delete_workspace(&system, workspace.id).await?;
        events.publish(GlobalEvent::WorkspaceDeleted(workspace.id), workspace.security_attributes.clone());
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
    for personality in deleted_with(store.list_deleted_personalities(&system).await?, holder, since) {
        unless_taken(store.restore_personality(&system, personality.id).await, "personality", &personality.name)?;
    }
    for template in deleted_with(store.list_deleted_session_templates(&system).await?, holder, since) {
        unless_taken(store.restore_session_template(&system, template.id).await, "session template", &template.name)?;
    }
    for agent in deleted_with(store.list_deleted_agents(&system).await?, holder, since) {
        if unless_taken(store.restore_agent(&system, agent.id).await, "agent", &agent.name)? {
            events.publish(GlobalEvent::AgentRestored(agent.id), agent.security_attributes.clone());
        }
    }
    for workspace in deleted_with(store.list_deleted_workspaces(&system).await?, holder, since) {
        if unless_taken(store.restore_workspace(&system, workspace.id).await, "workspace", &workspace.name)? {
            events.publish(GlobalEvent::WorkspaceRestored(workspace.id), workspace.security_attributes.clone());
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
/// agents' state with them, as `SessionManager::purge_agent` does, and
/// the workspaces' places.
///
/// # Errors
/// Whatever the stores or the workspaces' storages report.
pub(crate) async fn purge_scoped_in(store: &dyn EntityStore, states: &dyn StateStore, workspaces: &WorkspaceManager, holder: &Scope) -> anyhow::Result<()> {
    let system = SecurityContext::system();
    for personality in lying_in(store.list_deleted_personalities(&system).await?, holder) {
        store.purge_personality(&system, personality.id).await?;
    }
    for template in lying_in(store.list_deleted_session_templates(&system).await?, holder) {
        store.purge_session_template(&system, template.id).await?;
    }
    for agent in lying_in(store.list_deleted_agents(&system).await?, holder) {
        store.purge_agent(&system, agent.id).await?;
        states.delete_agent_state(agent.id).await?;
    }
    workspaces.purge_lying_in(holder).await?;
    Ok(())
}

/// Those of `scoped` that lie in `holder` or within it.
fn lying_in<D: Scoped>(scoped: Vec<D>, holder: &Scope) -> Vec<D> {
    scoped.into_iter().filter(|d| holder.contains(&d.scoped_name().scope)).collect()
}

/// Those of `scoped` that lie in `holder` or within it and were
/// deleted at `since` or later.
fn deleted_with<D: Scoped + Stored>(scoped: Vec<D>, holder: &Scope, since: DateTime<Utc>) -> Vec<D> {
    lying_in(scoped, holder).into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
