//! `AgentManager`: making, deleting, restoring and purging agents (issues
//! 227, 314) - their lifecycle. What an agent is - its name - is changed by
//! its `AgentController`.
//!
//! The `AgentManager` is the owner of an agent on its own - one of a
//! user's or a group's scope, or a global one, that outlives every session
//! it takes part in - and of those that lie in a session's or a channel's
//! scope, which go with it (`scoped`) and which it deletes, restores and
//! purges with it (`…_lying_in`).
//!
//! A deleted agent rests in the channels it takes part in (issue 314): their
//! documents keep it, and they follow its mark instead - a running channel
//! stops its actor and starts it again once it is restored (`SessionLoop`,
//! `ChannelLoop::follow_agent`), a channel that starts leaves it out, and
//! the views show it as a former member, removed as its mark says.

use crate::event::EventBus;
use crate::scoped::{ScopeMember, deleted_since, deleted_unless_refused, restored_unless_refused, within};
use async_trait::async_trait;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::event::GlobalEvent;
use caelumora_model::removal::Stored;
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::store::{EntityKind, EntityStore, EntityStores, StateStore, StoreError, StoreResult, StoreResultExt};
use caelumora_model::value::ValueError;
use caelumora_model::value::name::{Name, ScopedName};
use chrono::{DateTime, Utc};
use std::fmt;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No such agent, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no agent {0}")]
    NotFound(AgentId),
    /// The personality of an agent lies in a session's or a channel's scope
    /// the agent would not lie in (issue 178).
    #[error("the personality '{0}' cannot make an agent outside the session or channel it lies in")]
    PersonalityOutOfScope(Name),
    /// What a value was given as does not fit it: the name of an agent to
    /// make.
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type AgentResult<T> = Result<T, AgentError>;

/// `error` of the store about agent `id`: not found is one case with not
/// readable.
fn store_error(id: AgentId) -> impl FnOnce(StoreError) -> AgentError {
    move |error| match error {
        StoreError::NotFound { .. } => AgentError::NotFound(id),
        error => AgentError::Store(error),
    }
}

/// The one owner of agents' lifecycle (issue 314).
pub struct AgentManager {
    agents: Arc<dyn EntityStore<Agent>>,
    personalities: Arc<dyn EntityStore<Personality>>,
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
    pub(crate) fn new(stores: &EntityStores, states: Arc<dyn StateStore>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>) -> Self {
        Self { agents: Arc::clone(&stores.agents), personalities: Arc::clone(&stores.personalities), states, events, security }
    }

    /// Makes an agent of the personality `personality_id`, of `name` in
    /// `scope`, on behalf of `context`, whose principal owns it (`CreateAgent`),
    /// and announces it. Making it is asked before the personality is read,
    /// in `context`'s name (`GetPersonality`, issue 221): who may not make the
    /// agent learns nothing of the personality.
    ///
    /// # Errors
    /// `Invalid` for a name that does not fit, `PersonalityOutOfScope` if
    /// the personality lies in a session's or a channel's scope the agent
    /// would not; a `StoreError` -
    /// `PermissionDenied` if they may not make it or not read the
    /// personality, `NotFound` if there is no such personality, `NameTaken`
    /// if an agent of `name` lies in its scope already (issue 250).
    pub async fn create(&self, context: &SecurityContext, name: &str, scope: Scope, personality_id: PersonalityId) -> AgentResult<Agent> {
        let agent = Agent::new(ScopedName::new(name, scope)?, personality_id, context.principal_id);
        self.security
            .require(context, ActionType::create(Agent::KIND, &agent.security_attributes), StoreError::PermissionDenied).await?;
        let personality = self.personalities.get(context, personality_id).await?;
        if !personality.name.scope.admits(&agent.name.scope) {
            return Err(AgentError::PersonalityOutOfScope(personality.name.name));
        }
        self.agents.create(context, &agent).await?;
        self.events.publish(GlobalEvent::created::<Agent>(&agent.id), agent.security_attributes.clone());
        info!("Agent {} created by {}", agent.id, context.caller_id);
        Ok(agent)
    }

    /// Deletes agent `id` on behalf of `context` (issues 164, 314): marked
    /// deleted, with its state kept until it is purged. Asked about before
    /// anything happens (`DeleteAgent`). `false` if there was none the
    /// caller could see.
    ///
    /// The channels it takes part in keep it: it rests in them, without an
    /// actor, and is shown as a former member until it is restored. A
    /// running channel stops its actor as it learns of the deletion
    /// (`SessionLoop`), a channel that starts leaves it out; what it said
    /// stays in the histories.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may read it but not delete
    /// it, or whatever else the store reports.
    pub async fn delete(&self, context: &SecurityContext, id: AgentId) -> AgentResult<bool> {
        let Some(agent) = self.agents.get(context, id).await.optional()? else { return Ok(false) };
        self.security
            .require(context, ActionType::delete(Agent::KIND, &agent.security_attributes), StoreError::PermissionDenied).await?;
        if !self.agents.delete(context, id).await? {
            return Ok(false);
        }
        self.events.publish(GlobalEvent::deleted::<Agent>(&id), agent.security_attributes);
        info!("Agent {} deleted by {}", id, context.caller_id);
        Ok(true)
    }

    /// Restores the deleted agent `id` on behalf of `context` (issues 164,
    /// 314): its mark taken away, with its state. It takes part again in
    /// every channel it rested in - a running one starts its actor as it
    /// learns of it (`SessionLoop`). Returns the agent as it is restored.
    ///
    /// # Errors
    /// `NotFound` if there is no such deleted agent or `context` may not see
    /// it, `StoreError::PermissionDenied` if they may but not restore it,
    /// `StoreError::NameTaken` if another agent took its name in its scope
    /// meanwhile (issue 250).
    pub async fn restore(&self, context: &SecurityContext, id: AgentId) -> AgentResult<Agent> {
        let agent = self.agents.restore(context, id).await.map_err(store_error(id))?;
        self.events.publish(GlobalEvent::restored::<Agent>(&id), agent.security_attributes.clone());
        info!("Agent {} restored by {}", id, context.caller_id);
        Ok(agent)
    }

    /// Removes the deleted agent `id` for good on behalf of `context` (issue
    /// 164): its document, its own state and its state in every channel.
    /// Only what was deleted is purged; the store asks `Purge`. Announces
    /// nothing, as no purge does. What it said stays in the channels'
    /// histories, by an agent that no longer exists; the channels it rested
    /// in show it as a former member, without when it left. `false` if
    /// there was nothing deleted the caller could see.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may see it but not purge it,
    /// or whatever else the stores report.
    pub async fn purge(&self, context: &SecurityContext, id: AgentId) -> AgentResult<bool> {
        if !self.agents.purge(context, id).await? {
            return Ok(false);
        }
        self.states.delete_agent_state(id).await?;
        info!("Agent {} purged by {}", id, context.caller_id);
        Ok(true)
    }
}

#[async_trait]
impl ScopeMember for AgentManager {
    fn kind(&self) -> EntityKind {
        Agent::KIND
    }

    /// Deletes the agents that lie in `holder` or within it on behalf of
    /// `ctx` (`scoped`), as far as they may, and announces them as `delete`
    /// does; answers how many are left. They leave no channel: they take
    /// part only within `holder` (`Scope::admits`).
    ///
    /// # Errors
    /// Whatever else the store reports; what was deleted before stays
    /// deleted.
    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize> {
        let system = SecurityContext::system();
        let mut left = 0;
        for agent in self.agents.list(&system, &within(holder)).await?.items {
            if deleted_unless_refused(self.agents.delete(ctx, agent.id).await)? {
                self.events.publish(GlobalEvent::deleted::<Agent>(&agent.id), agent.security_attributes);
            } else {
                left += 1;
            }
        }
        Ok(left)
    }

    /// Restores the agents that lie in `holder` or within it and were
    /// deleted at `since` or later on behalf of `ctx` (`scoped`), and
    /// announces them as `restore` does. One whose name was taken meanwhile
    /// stays deleted, as does one `ctx` may not restore.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        let system = SecurityContext::system();
        for agent in deleted_since(self.agents.list_deleted(&system, &within(holder)).await?.items, since) {
            if let Some(restored) = restored_unless_refused(self.agents.restore(ctx, agent.id).await, &agent)? {
                self.events.publish(GlobalEvent::restored::<Agent>(&restored.id), restored.security_attributes);
            }
        }
        Ok(())
    }

    /// Purges the deleted agents that lie in `holder` or within it, with
    /// their state, as `purge` does - in the runtime's name (`scoped`).
    ///
    /// # Errors
    /// Whatever the stores report; what was purged before stays purged.
    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        for agent in self.agents.list_deleted(&system, &within(holder)).await?.items {
            self.agents.purge(&system, agent.id).await?;
            self.states.delete_agent_state(agent.id).await?;
        }
        Ok(())
    }
}
