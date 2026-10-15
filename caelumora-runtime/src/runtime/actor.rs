use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::ComponentRegistry;
use crate::controller::SessionController;
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::security::SecurityAttributes;
use caelumora_model::session::SessionId;
use crate::agent::state::AgentStateHandle;
use crate::event::{EventSubscription, SubscriptionTopic};
use crate::view::ViewsFactory;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::session::Session;
use caelumora_model::store::StoreResult;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::Arc;
use super::{ChannelHandle, ChannelRuntime, Services};

/// What an actor runs with (issue 69): the services every actor needs, and
/// access to what concerns this actor alone - its controller, its state, its
/// events, a narrow view of its channel. It is made for one actor
/// (`ChannelRuntime::new_actor_runtime`) and hands out nothing about others.
/// It is the actor's alone: its components get only what they need of it
/// (`ComponentContext`, issue 128); what they read of the session they read
/// through their turn's views (`views()`, issue 163), as the
/// confidentiality rules allow. The full channel and session are
/// `pub(crate)`, for the user's commands and the views.
#[derive(Debug, Clone)]
pub struct ActorRuntime {
    pub(super) runtime: ChannelRuntime,
    pub(super) actor: ActorId,
    pub(super) session_id: SessionId,
    pub(super) channel_id: ChannelId,
}

impl ActorRuntime {
    /// The actor this runtime is for.
    pub fn actor_id(&self) -> ActorId {
        self.actor
    }

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> { self.runtime.runtime_manager.security_manager() }

    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { self.runtime.runtime_manager.confidentiality() }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { self.runtime.runtime_manager.secret_manager() }

    pub fn services(&self) -> &Services { self.runtime.runtime_manager.services() }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> {
        self.runtime.runtime_manager.component_registry()
    }

    /// What the system restricts of tool calls everywhere (issue 13).
    pub fn approval_rules(&self) -> &Arc<[ApprovalRule]> {
        self.runtime.runtime_manager.approval_rules()
    }

    /// The controller of the actor's session: what its agent's tool calls
    /// are guarded by, beside the channel's (issue 13).
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> {
        self.runtime.session_controller()
    }

    /// The user commands of the actor's session. Only a `UserActor` runs
    /// them: commands are for people, agents act through tools.
    pub fn command_executor(&self) -> &Arc<CommandExecutor<dyn ChannelCommandProvider>> {
        &self.runtime.command_executor
    }

    /// The attributes of the agent `id`, as stored - what is asked of an
    /// agent named in an event (`ControllerManager::agent_attributes`).
    ///
    /// # Errors
    /// `StoreError::NotFound` if the agent no longer exists, or whatever
    /// else the store reports.
    pub(crate) async fn agent_attributes(&self, id: AgentId) -> StoreResult<SecurityAttributes> {
        self.runtime.runtime_manager.controller_manager().agent_attributes(id).await
    }

    /// The session `id` as stored, as `security` may read it.
    pub async fn load_session(&self, security: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.runtime.runtime_manager.entity_stores().sessions.get(security, id).await
    }

    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62); `attributes` are the agent's.
    ///
    /// # Errors
    /// If this actor is no agent, or the state cannot be loaded.
    pub async fn agent_state(&self, attributes: SecurityAttributes) -> Result<Arc<AgentStateHandle>> {
        let agent = self.agent_id()?;
        self.runtime.runtime_manager.agent_states().get(agent, attributes).await
    }

    /// The state of this agent in its channel, as stored - of either kind
    /// (issue 206).
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports.
    pub async fn load_channel_state(&self) -> Result<AgentChannelState> {
        let agent = self.agent_id()?;
        Ok(self.runtime.runtime_manager.state_store().get_agent_channel_state(agent, self.channel_id).await?)
    }

    /// Stores the state of this agent in its channel, and marks it stored
    /// (`StateStore`).
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports.
    pub async fn store_channel_state(&self, state: &mut AgentChannelState) -> Result<()> {
        let agent = self.agent_id()?;
        Ok(self.runtime.runtime_manager.state_store().put_agent_channel_state(agent, self.channel_id, state).await?)
    }

    /// The global events about the actor's channel (issue 69): nothing about
    /// other channels can be subscribed to. Changes to the actor's own user
    /// or agent are the session's to take in, into the controller the actor
    /// shares (issue 120).
    pub fn subscribe_channel(&self) -> EventSubscription {
        self.runtime.runtime_manager.event_bus().subscribe(vec![SubscriptionTopic::Channel(self.channel_id)])
    }

    /// Announces that this agent's state in its channel changed and is
    /// stored (issue 65), with the channel's attributes and without content.
    /// Nothing once the channel is gone.
    pub fn announce_channel_state_changed(&self) {
        let (Ok(agent), Some(channel)) = (self.agent_id(), self.channel_loop()) else { return };
        self.runtime.runtime_manager.event_bus()
            .announce(GlobalEvent::custom::<Channel>(&(self.session_id, self.channel_id), channel::AGENT_STATE_CHANGED, &agent), &**channel.channel());
    }

    /// What the actor's turns navigate (issue 163): its session, read from
    /// its channel, asked in the context of each turn (issue 255).
    pub fn views(&self) -> ViewsFactory {
        ViewsFactory::of_actor(self.clone())
    }

    /// The channel the actor is in, or `None` while the channel is still
    /// being built or once it is gone.
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel.get()
    }

    /// The channel the actor is in, for what the actor is built with and
    /// reaches it later (`ToolExecutor`).
    pub(crate) fn channel_handle(&self) -> ChannelHandle {
        self.runtime.channel.clone()
    }

    fn agent_id(&self) -> Result<AgentId> {
        self.actor.as_agent_id().ok_or_else(|| anyhow!("{} is no agent", self.actor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeManager;
    use crate::testing::test_registry;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::config::AppConfig;
    use caelumora_model::user::UserId;
    use caelumora_model::confidentiality::ConfidentialityRules;

    fn channel_runtime() -> ChannelRuntime {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let manager = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default()), &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&store))).unwrap());
        let commands = Arc::new(CommandExecutor::empty(Arc::clone(manager.security_manager())));
        ChannelRuntime::outside_a_session(manager, commands, SessionId::new())
    }

    #[tokio::test]
    async fn an_actor_runtime_acts_for_its_own_actor_only() {
        let runtime = channel_runtime();
        let hers = runtime.new_actor_runtime(ActorId::User(UserId::new()), SessionId::new(), ChannelId::new());

        assert!(hers.load_channel_state().await.is_err(), "a user has no agent state");
        assert!(hers.agent_state(Default::default()).await.is_err());

        let its = runtime.new_actor_runtime(ActorId::Agent(AgentId::new()), SessionId::new(), ChannelId::new());
        its.store_channel_state(&mut AgentChannelState::default()).await.expect("its own state in its channel");
        its.load_channel_state().await.expect("and back");
    }
}
