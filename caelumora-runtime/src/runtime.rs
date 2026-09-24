use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, CommandProvider};
use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry, ComponentResult};
use crate::component_impl::{command, context, history, mcp, tool};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::llm::LLMClient;
use crate::llm_impl::ollama::OllamaClient;
use crate::moderator::Moderator;
use crate::moderator_impl::broadcast::BroadcastModerator;
use crate::moderator_impl::random::RandomModerator;
use crate::moderator_impl::round_robin::RoundRobinModerator;
use crate::recorder::Recorder;
use crate::tool::{DefaultToolExecutor, ToolExecutor, ToolProvider};
use anyhow::Result;
use caelumora_model::actor::ActorId;
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
    llm_client: Arc<dyn LLMClient>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
    commands: ComponentConfig,
}

/// What a channel runs with: the process-wide services of the
/// `RuntimeManager`, the user commands of its session (issue 28) and a handle
/// to the channel itself. It is the only source of `ActorRuntime`s, so every
/// actor lives in exactly one channel and shares its commands and handle.
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

    /// The runtime of an actor of this channel.
    pub fn new_actor_runtime(&self) -> ActorRuntime {
        ActorRuntime { runtime: self.clone() }
    }

    /// Makes `channel` reachable from the runtimes of its actors. Called once
    /// by `ChannelLoop::new`, as soon as the channel exists.
    pub(crate) fn attach(&self, channel: &Arc<ChannelLoop>) {
        let _ = self.channel.set(Arc::downgrade(channel));
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.runtime_manager.security_manager() }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.runtime_manager.state_store() }

    pub fn event_bus(&self) -> Arc<EventBus> { self.runtime_manager.event_bus() }

    pub fn controller_manager(&self) -> Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    pub fn new_moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> { self.runtime_manager.new_moderator(kind) }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }
}

/// What an actor runs with: the process-wide services, and the commands and
/// the channel of the `ChannelRuntime` that created it.
#[derive(Debug, Clone)]
pub struct ActorRuntime {
    runtime: ChannelRuntime,
}

impl ActorRuntime {
    pub fn event_bus(&self) -> Arc<EventBus> {
        self.runtime.runtime_manager.event_bus()
    }

    pub fn security_manager(&self) -> Arc<dyn SecurityManager> { self.runtime.runtime_manager.security_manager() }

    pub fn llm_client(&self) -> Arc<dyn LLMClient> {
        self.runtime.runtime_manager.llm_client.clone()
    }

    pub fn state_store(&self) -> Arc<dyn StateStore> { self.runtime.runtime_manager.state_store() }

    pub fn entity_store(&self) -> Arc<dyn EntityStore> { self.runtime.runtime_manager.entity_store() }

    pub fn secret_manager(&self) -> Arc<SecretManager> { self.runtime.runtime_manager.secret_manager() }

    pub fn component_registry(&self) -> Arc<ComponentRegistry> {
        self.runtime.runtime_manager.component_registry()
    }

    pub fn controller_manager(&self) -> Arc<ControllerManager> {
        self.runtime.runtime_manager.controller_manager()
    }

    pub fn new_tool_executor(&self, tool_providers: Vec<Arc<dyn ToolProvider>>) -> Box<dyn ToolExecutor> {
        Box::new(DefaultToolExecutor::new(self.security_manager(), tool_providers))
    }

    /// The user commands of the actor's session. Only a `UserActor` runs
    /// them: commands are for people, agents act through tools.
    pub fn command_executor(&self) -> Arc<CommandExecutor> {
        self.runtime.command_executor.clone()
    }

    /// The channel the actor is in, or `None` while the channel is still
    /// being built or once it is gone.
    pub fn channel(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel.get().and_then(Weak::upgrade)
    }
}

impl RuntimeManager {
    /// The runtime everything in a process shares. `security_manager` is the
    /// one the whole process was configured with: it decides tool calls and
    /// what may be done to a channel, so it must be the same instance the
    /// stores were built with, not a second one made up here. `commands` are
    /// the global user commands (`AppConfig::commands`) - likewise an
    /// argument, so that a binary cannot drop the configured ones for a
    /// default.
    pub fn new(
        state_store: Arc<dyn StateStore>,
        entity_store: Arc<dyn EntityStore>,
        secret_manager: Arc<SecretManager>,
        security_manager: Arc<dyn SecurityManager>,
        commands: ComponentConfig,
    ) -> Result<Self> {
        let mut component_registry = ComponentRegistry::new(entity_store.clone());
        history::register(&mut component_registry);
        context::register(&mut component_registry);
        tool::register(&mut component_registry);
        mcp::register(&mut component_registry);
        command::register(&mut component_registry);

        let event_bus = Arc::new(EventBus::new());
        let controller_manager = Arc::new(ControllerManager::new(entity_store.clone(), event_bus.clone()));

        Ok(Self {
            security_manager,
            state_store,
            entity_store,
            secret_manager,
            llm_client: Arc::new(OllamaClient::default()),
            component_registry: Arc::new(component_registry),
            event_bus,
            controller_manager,
            commands,
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
        let context = ComponentContext::new(security, self.secret_manager());
        let mut providers = Vec::new();
        // Each configuration on its own, so that component ids only need to
        // be unique within one of them.
        for config in [&self.commands, &session.commands] {
            let container = ComponentContainer::from_config(config, &self.component_registry, &context).await?;
            providers.extend(container.get_all::<dyn CommandProvider>(None)?);
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

    pub fn new_moderator(&self, kind: &String) -> Result<Box<dyn Moderator>> {
        match kind.as_str() {
            "broadcast" => Ok(Box::new(BroadcastModerator::new())),
            "random" => Ok(Box::new(RandomModerator::new())),
            "round_robin" => Ok(Box::new(RoundRobinModerator::new())),
            _ => Err(anyhow::anyhow!("Unsupported moderator type: {}", kind))
        }
    }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(crate::recorder::BlackholeRecorder::new()))
    }
}
