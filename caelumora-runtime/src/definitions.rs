//! What lies in a scope goes with what holds it (issue 178): the
//! personalities, session templates and agents of a session's scope - and
//! of the scopes of its channels - are deleted with the session, restored
//! with it and purged with it; those of a channel's scope are deleted and
//! restored with the channel. `SessionManager` and `SessionLoop` are the
//! callers.
//!
//! All of it is done in the runtime's name: deleting, restoring or purging
//! what holds the scope was permitted, and what lies in it may be another's
//! to delete by the policy - a contributor's agent in the owner's session.

use crate::event::EventBus;
use caelumora_model::agent::AgentId;
use caelumora_model::event::GlobalEvent;
use caelumora_model::removal::Stored;
use caelumora_model::scope::{Definition, Scope};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StateStore, StoreResult};
use chrono::{DateTime, Utc};

/// Deletes what lies in `holder` or within it, and announces the agents
/// among it as `SessionManager::delete_agent` does. Returns the agents, so
/// that whoever holds their channels can have them leave.
///
/// # Errors
/// Whatever the store reports; what was deleted before stays deleted.
pub(crate) async fn delete_definitions_of(store: &dyn EntityStore, events: &EventBus, holder: &Scope) -> StoreResult<Vec<AgentId>> {
    let system = SecurityContext::system();
    for personality in lying_in(store.list_personalities(&system).await?, holder) {
        store.delete_personality(&system, personality.id).await?;
    }
    for template in lying_in(store.list_session_templates(&system).await?, holder) {
        store.delete_session_template(&system, template.id).await?;
    }
    let mut agents = Vec::new();
    for agent in lying_in(store.list_agents(&system).await?, holder) {
        store.delete_agent(&system, agent.id).await?;
        events.publish(GlobalEvent::AgentDeleted(agent.id), agent.security_attributes.clone());
        agents.push(agent.id);
    }
    Ok(agents)
}

/// Restores what lies in `holder` or within it and was deleted with it -
/// at `since`, when `holder` was deleted, or later. What was deleted
/// before stays deleted, as a channel deleted before its session does.
///
/// # Errors
/// Whatever the store reports; what was restored before stays restored.
pub(crate) async fn restore_definitions_of(store: &dyn EntityStore, events: &EventBus, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
    let system = SecurityContext::system();
    for personality in deleted_with(store.list_deleted_personalities(&system).await?, holder, since) {
        store.restore_personality(&system, personality.id).await?;
    }
    for template in deleted_with(store.list_deleted_session_templates(&system).await?, holder, since) {
        store.restore_session_template(&system, template.id).await?;
    }
    for agent in deleted_with(store.list_deleted_agents(&system).await?, holder, since) {
        store.restore_agent(&system, agent.id).await?;
        events.publish(GlobalEvent::AgentRestored(agent.id), agent.security_attributes.clone());
    }
    Ok(())
}

/// Purges what lies in `holder` or within it and is deleted, and the
/// agents' state with them, as `SessionManager::purge_agent` does.
///
/// # Errors
/// Whatever the stores report.
pub(crate) async fn purge_definitions_of(store: &dyn EntityStore, states: &dyn StateStore, holder: &Scope) -> StoreResult<()> {
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
    Ok(())
}

/// Those of `definitions` that lie in `holder` or within it.
fn lying_in<D: Definition>(definitions: Vec<D>, holder: &Scope) -> Vec<D> {
    definitions.into_iter().filter(|d| holder.contains(&d.scoped_name().scope)).collect()
}

/// Those of `definitions` that lie in `holder` or within it and were
/// deleted at `since` or later.
fn deleted_with<D: Definition + Stored>(definitions: Vec<D>, holder: &Scope, since: DateTime<Utc>) -> Vec<D> {
    lying_in(definitions, holder).into_iter().filter(|d| d.deleted().is_some_and(|removal| removal.at >= since)).collect()
}
