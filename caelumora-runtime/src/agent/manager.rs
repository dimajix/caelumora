//! `AgentManager`: deleting, restoring and purging agents (issue 314).
//!
//! What lies in a session's or a channel's scope goes with it (`scoped`),
//! its agents among it; the `AgentManager` is the owner of an agent on its
//! own - one of a user's or a group's scope, or a global one, that outlives
//! every session it takes part in.

use crate::event::EventBus;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::store::{EntityStore, StateStore, StoreError, StoreResult};
use std::fmt;
use std::sync::Arc;
use tracing::info;

/// The one owner of agents' lifecycle (issue 314).
pub struct AgentManager {
    entities: Arc<dyn EntityStore>,
    states: Arc<dyn StateStore>,
    events: Arc<EventBus>,
    security: Arc<dyn SecurityManager>,
}

impl fmt::Debug for AgentManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentManager").finish_non_exhaustive()
    }
}

impl AgentManager {
    pub(crate) fn new(entities: Arc<dyn EntityStore>, states: Arc<dyn StateStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>) -> Self {
        Self { entities, states, events, security }
    }

    /// Deletes agent `id` on behalf of `context` (issue 164): marked
    /// deleted, with its state kept until it is purged. Asked about before
    /// anything happens (`DeleteAgent`). What it said stays in the channels'
    /// histories.
    ///
    /// The channels it takes part in are not changed here: that it leaves
    /// them is `SessionManager::delete_agent`'s, which calls this first.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such agent or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may but not
    /// delete it.
    pub async fn delete(&self, context: &SecurityContext, id: AgentId) -> StoreResult<()> {
        let agent = self.entities.get_agent(context, id).await?;
        self.security
            .require(context, ActionType::DeleteAgent(&agent.security_attributes), || StoreError::PermissionDenied).await?;
        self.entities.delete_agent(context, id).await?;
        self.events.publish(GlobalEvent::AgentDeleted(id), agent.security_attributes);
        info!("Agent {} deleted by {}", id, context.caller_id);
        Ok(())
    }

    /// Restores the deleted agent `id` on behalf of `context` (issue 164):
    /// its mark taken away, with its state. It takes part in no channel
    /// again - it left them when it was deleted - until it is added.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted agent or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not restore it, `StoreError::NameTaken` if another agent took its
    /// name in its scope meanwhile (issue 250).
    pub async fn restore(&self, context: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        let deleted = self.entities.get_deleted_agent(context, id).await?;
        self.entities.restore_agent(context, id).await?;
        let agent = Agent { deleted: None, ..deleted };
        self.events.publish(GlobalEvent::AgentRestored(id), agent.security_attributes.clone());
        info!("Agent {} restored by {}", id, context.caller_id);
        Ok(agent)
    }

    /// Removes the deleted agent `id` for good on behalf of `context` (issue
    /// 164): its document, its own state and its state in every channel.
    /// Only what was deleted is purged; the store asks `Purge`. What it
    /// said stays in the channels' histories, by an agent that no longer
    /// exists.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted agent or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not purge it.
    pub async fn purge(&self, context: &SecurityContext, id: AgentId) -> StoreResult<()> {
        if !self.entities.purge_agent(context, id).await? {
            return Err(StoreError::NotFound { entity: "agent", id: id.to_string() });
        }
        self.states.delete_agent_state(id).await?;
        info!("Agent {} purged by {}", id, context.caller_id);
        Ok(())
    }
}
