//! `AgentManager`: making, deleting, restoring and purging agents (issues
//! 227, 314) - their lifecycle. What an agent is - its name - is changed by
//! its `AgentController`.
//!
//! The `AgentManager` is the owner of an agent on its own - one of a
//! user's or a group's scope, or a global one, that outlives every session
//! it takes part in - and of those that lie in a session's or a channel's
//! scope, which go with it (`scoped`): as their `ScopeMember`, it says how
//! they are announced and what is purged with them.
//!
//! A deleted agent rests in the channels it takes part in (issue 314): their
//! documents keep it, and they follow its mark instead - a running channel
//! stops its actor and starts it again once it is restored (`SessionLoop`,
//! `ChannelLoop::follow_agent`), a channel that starts leaves it out, and
//! the views show it as a former participant, removed as its mark says.

use super::{store_error, AgentError, AgentResult};
use crate::event::EventBus;
use crate::scoped::{ScopeHolders, ScopeMember};
use async_trait::async_trait;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use caelumora_model::store::{
    purge_behind, EntityStore, EntityStores, StateStore, StoreError, StoreResult, StoreResultExt,
    Stored,
};
use caelumora_model::value::name::ScopedName;
use std::fmt;
use std::sync::Arc;
use tracing::info;

/// The one owner of agents' lifecycle (issue 314).
pub struct AgentManager {
    agents: Arc<dyn EntityStore<Agent>>,
    personalities: Arc<dyn EntityStore<Personality>>,
    /// Asked whether an agent may be put into a scope.
    holders: Arc<ScopeHolders>,
    states: Arc<dyn StateStore>,
    events: Arc<EventBus>,
    access_policy: Arc<dyn AccessPolicy>,
}

impl fmt::Debug for AgentManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentManager").finish_non_exhaustive()
    }
}

impl AgentManager {
    pub(crate) fn new(
        stores: &EntityStores,
        holders: Arc<ScopeHolders>,
        states: Arc<dyn StateStore>,
        events: Arc<EventBus>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        Self {
            agents: Arc::clone(&stores.agents),
            personalities: Arc::clone(&stores.personalities),
            holders,
            states,
            events,
            access_policy,
        }
    }

    /// Makes an agent of the personality `personality_id`, of `name` in
    /// `scope`, on behalf of `context`, whose principal owns it (`CreateAgent`),
    /// and announces it. Making it is asked before anything is read, in
    /// `context`'s name: who may not make the agent learns nothing of the
    /// session or the personality.
    ///
    /// An agent of a session's or a channel's scope goes with it: what lies
    /// there and cannot be deleted by who deletes the session keeps the
    /// session from being deleted (`ScopeNotEmpty`). So only who may read
    /// the session (`GetSession`) - or the channel (`GetChannel`) - puts an
    /// agent there, as only who takes part puts anything else there. Then
    /// the personality is read (`GetPersonality`, issue 221).
    ///
    /// # Errors
    /// `Invalid` for a name that does not fit, `PersonalityOutOfScope` if
    /// the personality lies in a session's or a channel's scope the agent
    /// would not; a `StoreError` - `PermissionDenied` if they may not make
    /// it or not read the personality, `NotFound` if there is no such
    /// session, channel, project or personality they may read, `NameTaken`
    /// if an agent of `name` lies in its scope already (issue 250),
    /// `Validation` for a scope no agent lies in - a project's they may
    /// read -, or for the system as its owner (issue 477).
    pub async fn create(
        &self,
        context: &SecurityContext,
        name: &str,
        scope: Scope,
        personality_id: PersonalityId,
    ) -> AgentResult<Agent> {
        let mut agent =
            Agent::new(ScopedName::new(name, scope)?, personality_id, context.principal_id);
        self.access_policy
            .require(
                context,
                ActionType::create(Agent::KIND, &agent.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        self.holders.require(context, &agent.name.scope).await?;
        let personality = self.personalities.get(context, personality_id).await?;
        if !personality.name.scope.admits(&agent.name.scope) {
            return Err(AgentError::PersonalityOutOfScope(personality.name.name));
        }
        self.agents.create(context, &mut agent).await?;
        self.holders.keep_made(self, &agent).await?;
        self.events.announce(GlobalEvent::created::<Agent>(&agent.id), &agent);
        info!("Agent {} created by {}", agent.id, context.caller_id);
        Ok(agent)
    }

    /// Deletes agent `id` on behalf of `context` (issues 164, 314): marked
    /// deleted, with its state kept until it is purged. Asked about before
    /// anything happens (`DeleteAgent`). `false` if there was none the
    /// caller could see.
    ///
    /// The channels it takes part in keep it: it rests in them, without an
    /// actor, and is shown as a former participant until it is restored. A
    /// running channel stops its actor as it learns of the deletion
    /// (`SessionLoop`), a channel that starts leaves it out; what it said
    /// stays in the histories.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may read it but not delete
    /// it, or whatever else the store reports.
    pub async fn delete(&self, context: &SecurityContext, id: AgentId) -> AgentResult<bool> {
        let Some(agent) = self.agents.get(context, id).await.optional()? else { return Ok(false) };
        self.access_policy
            .require(
                context,
                ActionType::delete(Agent::KIND, &agent.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        if !self.agents.delete(context, id, &Removal::now(context.caller_id)).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<Agent>(&id), &agent);
        info!("Agent {} deleted by {}", id, context.caller_id);
        Ok(true)
    }

    /// Restores the deleted agent `id` on behalf of `context` (issues 164,
    /// 314): its mark taken away, with its state, if what holds its scope is
    /// still there (issue 458). It takes part again in every channel it
    /// rested in - a running one starts its actor as it learns of it
    /// (`SessionLoop`). Returns the agent as it is restored.
    ///
    /// # Errors
    /// `NotFound` if there is no such deleted agent, `context` may not see
    /// it or its purge is begun (issue 492), or if what holds its scope is
    /// gone, `StoreError::PermissionDenied`
    /// if they may but not restore it, `StoreError::NameTaken` if another
    /// agent took its name in its scope meanwhile (issue 250).
    pub async fn restore(&self, context: &SecurityContext, id: AgentId) -> AgentResult<Agent> {
        let agent = self.holders.restore(self, context, id).await.map_err(store_error(id))?;
        self.events.announce(GlobalEvent::restored::<Agent>(&id), &agent);
        info!("Agent {} restored by {}", id, context.caller_id);
        Ok(agent)
    }

    /// Removes the deleted agent `id` for good on behalf of `context` (issue 164), its state - its
    /// own and in every channel - first, in the order of every purge of what has something behind
    /// it (`purge_behind`). Only what was deleted is purged; the store asks `Purge`. A state that
    /// cannot be removed leaves the agent deleted, its purge begun, so that the next purge tries
    /// again. Announces nothing, as no purge does. What it said stays in the channels' histories,
    /// by an agent that no longer exists; the channels it rested in show it as a former
    /// participant, without when it left. `false` if there was nothing deleted the caller could
    /// see.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may see it but not purge it, or whatever else the
    /// stores report.
    pub async fn purge(&self, context: &SecurityContext, id: AgentId) -> AgentResult<bool> {
        let purged = purge_behind(self.agents.as_ref(), context, id, async |agent| {
            self.remove_behind(agent).await
        })
        .await?;
        if purged {
            info!("Agent {} purged by {}", id, context.caller_id);
        }
        Ok(purged)
    }
}

/// Agents go with what holds the scope they lie in, announced as `delete` and `restore` announce
/// them, and purged with their state (`scoped`). They leave no channel as they go: they take part
/// only within the scope they lie in (`Scope::admits`).
#[async_trait]
impl ScopeMember for AgentManager {
    type Entity = Agent;

    fn store(&self) -> &dyn EntityStore<Agent> {
        self.agents.as_ref()
    }

    fn events(&self) -> Option<&dyn EventPublisher> {
        Some(self.events.as_ref())
    }

    /// Removes the state of `agent`, its own and in every channel, once its purge is begun.
    ///
    /// # Errors
    /// Whatever the state store reports.
    async fn remove_behind(&self, agent: &Agent) -> StoreResult<()> {
        self.states.delete_agent_state(agent.id).await
    }
}
