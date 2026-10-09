use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry, ComponentResult};
use crate::controller::{ControllerManager, SessionController};
use crate::event::EventBus;
use crate::recorder::Recorder;
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::ChannelId;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::SecurityAttributes;
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreResult;
use crate::agent::state::AgentStateHandle;
use crate::event::{EventSubscription, SubscriptionTopic};
use crate::view::ViewsFactory;
use caelumora_model::component::ComponentConfig;
use caelumora_model::config::AppConfig;
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
    /// What the system restricts of tool calls everywhere
    /// (`AppConfig::approvals`, issue 13).
    approval_rules: Arc<[ApprovalRule]>,
    /// One owner of every running agent's `AgentState` (issue 62).
    agent_states: Arc<crate::agent::state::AgentStates>,
    /// The one owner of workspaces and their places (issue 253).
    workspaces: Arc<crate::workspace::WorkspaceManager>,
    /// The one writer of skills (issue 22).
    skills: Arc<crate::skill::SkillManager>,
}

/// What a channel runs with: the process-wide services of the
/// `RuntimeManager` - among them the controllers of its users and agents
/// (issue 121) - the user commands of its session (issue 28), the
/// session's controller (issue 13) and a handle to the channel itself. It
/// is the only source of `ActorRuntime`s, so every actor lives in exactly
/// one channel and shares its commands and handle.
#[derive(Debug, Clone)]
pub struct ChannelRuntime {
    runtime_manager: Arc<RuntimeManager>,
    command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
    /// The session the channel belongs to, live: what its agents' tool
    /// calls are guarded by (`GuardedToolExecutor`).
    session: Arc<SessionController>,
    channel: ChannelHandle,
}

/// The channel its actors run in, reachable from what they are built with
/// once it exists: set by `ChannelLoop::new` as soon as the channel does,
/// after its actors were built. Weak, because the channel owns its actors
/// and they hold this handle: a strong reference would keep a stopped
/// channel alive forever.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChannelHandle(Arc<OnceLock<Weak<ChannelLoop>>>);

impl ChannelHandle {
    /// The channel, or `None` while it is still being built or once it is
    /// gone.
    pub(crate) fn get(&self) -> Option<Arc<ChannelLoop>> {
        self.0.get().and_then(Weak::upgrade)
    }
}

impl ChannelRuntime {
    /// A runtime for a channel of the session `session` holds, whose user
    /// commands are `commands`.
    pub fn new(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session: Arc<SessionController>) -> Self {
        Self { runtime_manager, command_executor, session, channel: ChannelHandle::default() }
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
        let _ = self.channel.0.set(Arc::downgrade(channel));
    }

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> { self.runtime_manager.security_manager() }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { self.runtime_manager.secret_manager() }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> { self.runtime_manager.component_registry() }

    pub fn state_store(&self) -> &Arc<dyn StateStore> { self.runtime_manager.state_store() }

    pub fn event_bus(&self) -> &Arc<EventBus> { self.runtime_manager.event_bus() }

    pub fn controller_manager(&self) -> &Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    /// The entities - for what a channel checks of its participants (issue
    /// 191).
    pub fn entity_store(&self) -> &Arc<dyn EntityStore> { self.runtime_manager.entity_store() }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }

    /// The controller of the channel's session.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> { &self.session }
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

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> { self.runtime.runtime_manager.security_manager() }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { self.runtime.runtime_manager.secret_manager() }

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
        let attributes = channel.security_attributes();
        self.runtime.runtime_manager.event_bus()
            .publish(GlobalEvent::AgentChannelStateChanged(self.session_id, self.channel_id, agent), attributes);
    }

    /// What the actor's turns navigate (issue 163): its session, read from
    /// its channel, asked in the context of each turn (issue 255).
    pub fn views(&self) -> ViewsFactory {
        ViewsFactory::of_actor(self.clone())
    }

    pub(crate) fn runtime_manager(&self) -> &Arc<RuntimeManager> {
        &self.runtime.runtime_manager
    }

    /// The channel the actor is in, or `None` while the channel is still
    /// being built or once it is gone.
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel.get()
    }

    /// The channel the actor is in, for what the actor is built with and
    /// reaches it later (`GuardedToolExecutor`).
    pub(crate) fn channel_handle(&self) -> ChannelHandle {
        self.runtime.channel.clone()
    }

    fn agent_id(&self) -> Result<AgentId> {
        self.actor.as_agent_id().ok_or_else(|| anyhow!("{} is no agent", self.actor))
    }
}

/// A session that holds nothing but its id `session_id` - in tests.
#[cfg(test)]
pub(crate) fn bare_session(session_id: SessionId) -> Session {
    let now = chrono::Utc::now();
    Session {
        summary: None, briefing: None,
        id: session_id,
        name: caelumora_model::value::name::Name::new("s").unwrap(),
        participants: Vec::new(),
        channels: Vec::new(),
        commands: Default::default(),
        privacy: Default::default(),
        approval_rules: Vec::new(),
        workspace: None,
        created_at: now,
        updated_at: now,
        security_attributes: Default::default(),
        deleted: None,
    }
}

#[cfg(test)]
impl ChannelRuntime {
    /// A runtime for a channel of the session `session_id` that is not
    /// running - in tests: its session holds nothing but its id, and no
    /// rules.
    pub(crate) fn outside_a_session(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session_id: SessionId) -> Self {
        let session = runtime_manager.controller_manager().new_session_controller(bare_session(session_id));
        Self::new(runtime_manager, command_executor, session)
    }
}

impl RuntimeManager {
    /// The runtime everything in a process shares. `security_manager` is the
    /// one the whole process was configured with: it decides tool calls and
    /// what may be done to a channel, so it must be the same instance the
    /// stores were built with, not a second one made up here. Of `config`,
    /// the runtime takes what it runs by itself, so that a binary cannot
    /// drop a configured part for a default: the global user commands
    /// (`commands`), the components that replace built-in defaults of facets
    /// of which an agent has one (`component_defaults`, issue 99), what the
    /// system restricts of tool calls (`approvals`, issue 13) and the
    /// storages workspaces are made in (`workspaces`, issue 253).
    /// `component_registry` holds the component kinds agents and sessions
    /// are built from -
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
        config: &AppConfig,
        mut component_registry: ComponentRegistry,
    ) -> ComponentResult<Self> {
        component_registry.set_defaults(&config.component_defaults)?;

        let event_bus = Arc::new(EventBus::new(Arc::clone(&security_manager)));
        let controller_manager = Arc::new(ControllerManager::new(Arc::clone(&entity_store), Arc::clone(&event_bus)));
        let agent_states = Arc::new(crate::agent::state::AgentStates::new(Arc::clone(&state_store), Arc::<EventBus>::clone(&event_bus)));
        let workspaces = Arc::new(crate::workspace::WorkspaceManager::new(Arc::clone(&entity_store), Arc::clone(&event_bus), Arc::clone(&security_manager), &secret_manager, &config.workspaces)?);
        let skills = Arc::new(crate::skill::SkillManager::new(Arc::clone(&entity_store), Arc::clone(&event_bus), Arc::clone(&security_manager)));

        Ok(Self {
            security_manager,
            state_store,
            entity_store,
            secret_manager,
            component_registry: Arc::new(component_registry),
            event_bus,
            controller_manager,
            commands: config.commands.clone(),
            approval_rules: config.approvals.rules.clone().into(),
            agent_states,
            workspaces,
            skills,
        })
    }

    /// The executor for the user commands of `session`: the global ones and
    /// the session's own (issue 28), built once for all of its channels,
    /// under the security context of the session's owner.
    ///
    /// # Errors
    /// A `ComponentError` if a command component cannot be built or two
    /// provide the same command - the session does not start then.
    pub async fn new_command_executor(&self, session: &Session) -> ComponentResult<CommandExecutor<dyn ChannelCommandProvider>> {
        let owner: ActorId = session.security_attributes.owner_id;
        let security = SecurityContext::new(owner, session.id, Default::default());
        let context = ComponentContext::new(security, Arc::clone(self.security_manager()), Arc::clone(self.secret_manager()));
        let mut providers = Vec::new();
        // Each configuration on its own, so that component ids only need to
        // be unique within one of them.
        for config in [&self.commands, &session.commands] {
            let container = ComponentContainer::from_config(config, &self.component_registry, &context).await?;
            providers.extend(container.get_all::<dyn ChannelCommandProvider>(None));
        }
        CommandExecutor::new(Arc::clone(self.security_manager()), providers)
    }

    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> { &self.security_manager }

    pub fn state_store(&self) -> &Arc<dyn StateStore> { &self.state_store }

    pub fn entity_store(&self) -> &Arc<dyn EntityStore> { &self.entity_store }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { &self.secret_manager }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> {
        &self.component_registry
    }

    pub fn controller_manager(&self) -> &Arc<ControllerManager> {
        &self.controller_manager
    }

    /// What the system restricts of tool calls everywhere (issue 13).
    pub fn approval_rules(&self) -> &Arc<[ApprovalRule]> {
        &self.approval_rules
    }

    /// The owner of every running agent's `AgentState` (issue 62).
    pub fn agent_states(&self) -> &Arc<crate::agent::state::AgentStates> {
        &self.agent_states
    }

    /// The one owner of workspaces and their places (issue 253).
    pub fn workspace_manager(&self) -> &Arc<crate::workspace::WorkspaceManager> {
        &self.workspaces
    }

    /// The one writer of skills (issue 22).
    pub fn skill_manager(&self) -> &Arc<crate::skill::SkillManager> {
        &self.skills
    }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(crate::recorder::BlackholeRecorder::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;

    fn channel_runtime() -> ChannelRuntime {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let manager = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(DummySecurityManager::new()), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
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

    #[test]
    fn the_configuration_may_replace_the_context_provider_but_only_with_one() {
        let manager = |defaults: serde_json::Value| {
            let store = Arc::new(MemoryStore::new());
            let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(DummySecurityManager::new()), &caelumora_model::config::AppConfig { component_defaults: defaults, ..Default::default() }, crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store)))
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!([{"id": "context", "kind": "turn_context_provider"}])).is_ok());
        let refused = manager(serde_json::json!([{"id": "context", "kind": "calculator_tool"}])).expect_err("provides no context");
        assert!(refused.to_string().contains("exactly one facet"), "{refused}");
    }
}

