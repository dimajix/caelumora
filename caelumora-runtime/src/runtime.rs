use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, CommandProvider};
use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry, ComponentResult};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::recorder::Recorder;
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::external_agent::ExternalAgentChannelState;
use caelumora_model::channel::ChannelId;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::SecurityAttributes;
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreResult;
use crate::agent_state::AgentStateHandle;
use crate::event::{EventSubscription, SubscriptionTopic};
use crate::view::ViewsFactory;
use caelumora_model::component::ComponentConfig;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::session::Session;
use caelumora_model::store::{EntityStore, StateStore};
use std::sync::{Arc, OnceLock, Weak};

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    state_store: Arc<dyn StateStore>,
    entity_store: Arc<dyn EntityStore>,
    secret_manager: Arc<SecretManager>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
    commands: ComponentConfig,
    /// One owner of every running agent's `AgentState` (issue 62).
    agent_states: Arc<crate::agent_state::AgentStates>,
}

/// What a channel runs with: the process-wide services of the
/// `RuntimeManager` - among them the controllers of its users and agents
/// (issue 121) - the user commands of its session (issue 28) and a handle
/// to the channel itself. It is the only source of `ActorRuntime`s, so every actor lives in
/// exactly one channel and shares its commands and handle.
#[derive(Debug, Clone)]
pub struct ChannelRuntime {
    runtime_manager: Arc<RuntimeManager>,
    command_executor: Arc<CommandExecutor>,
    // Set once the channel exists (see `attach`). Weak, because the channel
    // owns its actors and they hold this runtime: a strong reference would
    // keep a stopped channel alive forever.
    channel: Arc<OnceLock<Weak<ChannelLoop>>>,
}

impl ChannelRuntime {
    /// A runtime for a channel of a session whose user commands are `commands`.
    pub fn new(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor>) -> Self {
        Self { runtime_manager, command_executor, channel: Arc::new(OnceLock::new()) }
    }

    /// The runtime of `actor` in the channel `channel_id` of session
    /// `session_id` - the channel this runtime is for, which is not attached
    /// yet while its actors are built.
    pub fn new_actor_runtime(&self, actor: ActorId, session_id: SessionId, channel_id: ChannelId) -> ActorRuntime {
        ActorRuntime { runtime: self.clone(), actor, session_id, channel_id }
    }

    /// Makes `channel` reachable from the runtimes of its actors. Called once
    /// by `ChannelLoop::new`, as soon as the channel exists.
    pub(crate) fn attach(&self, channel: &Arc<ChannelLoop>) {
        let _ = self.channel.set(Arc::downgrade(channel));
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.runtime_manager.security_manager() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.runtime_manager.secret_manager() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> { self.runtime_manager.component_registry() }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.runtime_manager.state_store() }


    pub fn event_bus(&self) -> Arc<EventBus> { self.runtime_manager.event_bus() }

    pub fn controller_manager(&self) -> Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    /// The entities - for what a channel checks of its participants (issue
    /// 191).
    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.runtime_manager.entity_store() }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }
}

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
    runtime: ChannelRuntime,
    actor: ActorId,
    session_id: SessionId,
    channel_id: ChannelId,
}

impl ActorRuntime {
    /// The actor this runtime is for.
    pub fn actor_id(&self) -> ActorId {
        self.actor
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.runtime.runtime_manager.security_manager() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.runtime.runtime_manager.secret_manager() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.runtime.runtime_manager.component_registry()
    }

    /// The user commands of the actor's session. Only a `UserActor` runs
    /// them: commands are for people, agents act through tools.
    pub fn command_executor(&self) -> Arc<CommandExecutor> {
        self.runtime.command_executor.clone()
    }

    /// The session `id` as stored, as `security` may read it.
    pub async fn load_session(&self, security: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.runtime.runtime_manager.entity_store().get_session(security, id).await
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

    /// The state of this agent in its channel, as stored.
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

    /// The state of this external agent in its channel, as stored (issue
    /// 179).
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports.
    pub async fn load_external_channel_state(&self) -> Result<ExternalAgentChannelState> {
        let agent = self.agent_id()?;
        Ok(self.runtime.runtime_manager.state_store().get_external_agent_channel_state(agent, self.channel_id).await?)
    }

    /// Stores the state of this external agent in its channel.
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports.
    pub async fn store_external_channel_state(&self, state: &mut ExternalAgentChannelState) -> Result<()> {
        let agent = self.agent_id()?;
        Ok(self.runtime.runtime_manager.state_store().put_external_agent_channel_state(agent, self.channel_id, state).await?)
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
        let attributes = channel.security_attributes();
        self.runtime.runtime_manager.event_bus()
            .publish(GlobalEvent::AgentChannelStateChanged(self.session_id, self.channel_id, agent), attributes);
    }

    /// What the actor's turns navigate (issue 163): its session, read from
    /// its channel, asked in `context` - the actor's own (issue 131).
    pub fn views(&self, context: SecurityContext) -> ViewsFactory {
        ViewsFactory::of_actor(self.clone(), context)
    }

    pub(crate) fn runtime_manager(&self) -> &Arc<RuntimeManager> {
        &self.runtime.runtime_manager
    }

    /// The channel the actor is in, or `None` while the channel is still
    /// being built or once it is gone.
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel.get().and_then(Weak::upgrade)
    }

    fn agent_id(&self) -> Result<AgentId> {
        self.actor.as_agent_id().ok_or_else(|| anyhow!("{} is no agent", self.actor))
    }
}

impl RuntimeManager {
    /// The runtime everything in a process shares. `security_manager` is the
    /// one the whole process was configured with: it decides tool calls and
    /// what may be done to a channel, so it must be the same instance the
    /// stores were built with, not a second one made up here. `commands` are
    /// the global user commands (`AppConfig::commands`) - likewise an
    /// argument, so that a binary cannot drop the configured ones for a
    /// default. `component_defaults` replace built-in defaults of facets of
    /// which an agent has one (`AppConfig::component_defaults`, issue 99),
    /// an argument for the same reason. `component_registry` holds the
    /// component kinds agents and sessions are built from -
    /// `ComponentRegistry::built_in`, to which a test may add stand-ins,
    /// such as a model of its own (issue 130); it is checked here.
    ///
    /// # Errors
    /// A `ComponentError` if a default kind of the registry is missing or
    /// does not provide its facet (`ComponentRegistry::verify`), or if
    /// `component_defaults` cannot replace a default (see
    /// `ComponentRegistry::set_defaults`) - the process does not start
    /// then.
    pub fn new(
        state_store: Arc<dyn StateStore>,
        entity_store: Arc<dyn EntityStore>,
        secret_manager: Arc<SecretManager>,
        security_manager: Arc<dyn SecurityManager>,
        commands: ComponentConfig,
        component_defaults: ComponentConfig,
        mut component_registry: ComponentRegistry,
    ) -> ComponentResult<Self> {
        component_registry.set_defaults(&component_defaults)?;

        let event_bus = Arc::new(EventBus::new(security_manager.clone()));
        let controller_manager = Arc::new(ControllerManager::new(entity_store.clone(), event_bus.clone()));
        let agent_states = Arc::new(crate::agent_state::AgentStates::new(state_store.clone(), event_bus.clone()));

        Ok(Self {
            security_manager,
            state_store,
            entity_store,
            secret_manager,
            component_registry: Arc::new(component_registry),
            event_bus,
            controller_manager,
            commands,
            agent_states,
        })
    }

    /// The executor for the user commands of `session`: the global ones and
    /// the session's own (issue 28), built once for all of its channels,
    /// under the security context of the session's owner.
    ///
    /// # Errors
    /// A `ComponentError` if a command component cannot be built or two
    /// provide the same command - the session does not start then.
    pub async fn new_command_executor(&self, session: &Session) -> ComponentResult<CommandExecutor> {
        let owner: ActorId = session.security_attributes.owner_id;
        let security = SecurityContext::new(owner, session.id, Default::default());
        let context = ComponentContext::new(security, self.security_manager(), self.secret_manager());
        let mut providers = Vec::new();
        // Each configuration on its own, so that component ids only need to
        // be unique within one of them.
        for config in [&self.commands, &session.commands] {
            let container = ComponentContainer::from_config(config, &self.component_registry, &context).await?;
            providers.extend(container.get_all::<dyn CommandProvider>(None));
        }
        CommandExecutor::new(self.security_manager(), providers)
    }

    pub fn event_bus(&self) -> Arc<EventBus> {
        self.event_bus.clone()
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.security_manager.clone() }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.state_store.clone() }

    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.entity_store.clone() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.secret_manager.clone() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.component_registry.clone()
    }

    pub fn controller_manager(&self) -> Arc<ControllerManager> {
        self.controller_manager.clone()
    }

    /// The owner of every running agent's `AgentState` (issue 62).
    pub fn agent_states(&self) -> Arc<crate::agent_state::AgentStates> {
        self.agent_states.clone()
    }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(crate::recorder::BlackholeRecorder::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::command::default_commands;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::UserId;

    fn channel_runtime() -> ChannelRuntime {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let manager = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let commands = Arc::new(CommandExecutor::empty(manager.security_manager()));
        ChannelRuntime::new(manager, commands)
    }

    #[tokio::test]
    async fn an_actor_runtime_acts_for_its_own_actor_only() {
        let runtime = channel_runtime();
        let hers = runtime.new_actor_runtime(ActorId::User(UserId::new()), SessionId::new(), ChannelId::new());

        assert!(hers.load_channel_state().await.is_err(), "a user has no agent state");
        assert!(hers.agent_state(Default::default()).await.is_err());
        assert!(hers.load_external_channel_state().await.is_err(), "nor an external agent's");

        let its = runtime.new_actor_runtime(ActorId::Agent(AgentId::new()), SessionId::new(), ChannelId::new());
        its.store_channel_state(&mut AgentChannelState::default()).await.expect("its own state in its channel");
        its.load_channel_state().await.expect("and back");
        its.store_external_channel_state(&mut ExternalAgentChannelState::default()).await.expect("an external agent's state in its channel");
        its.load_external_channel_state().await.expect("and back");
    }

    #[test]
    fn the_configuration_may_replace_the_context_provider_but_only_with_one() {
        let manager = |defaults: serde_json::Value| {
            let store = Arc::new(MemoryStore::new());
            let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            RuntimeManager::new(store.clone(), store.clone(), secrets, Arc::new(DummySecurityManager::new()), default_commands(), defaults, crate::component::ComponentRegistry::built_in(store.clone()))
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!({"component_nodes": [{"id": "context", "kind": "state_context_provider"}]})).is_ok());
        let refused = manager(serde_json::json!({"component_nodes": [{"id": "context", "kind": "calculator_tool"}]})).err().expect("provides no context");
        assert!(refused.to_string().contains("exactly one facet"), "{refused}");
    }
}

