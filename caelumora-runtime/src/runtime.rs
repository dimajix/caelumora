use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry, ComponentResult};
use crate::controller::{ControllerManager, SessionController};
use crate::session::SessionLoop;
use crate::event::EventBus;
use crate::recorder::{BlackholeRecorder, Recorder};
use anyhow::{anyhow, bail, Context, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::security::SecurityAttributes;
use caelumora_model::session::SessionId;
use crate::agent::state::{AgentStateHandle, AgentStates};
use crate::event::{EventSubscription, SubscriptionTopic};
use crate::view::ViewsFactory;
use caelumora_model::component::ComponentConfig;
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::secret::{SecretManager, SecretProvider};
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::user_provider::{UserManager, UserProvider};
use crate::backend::Backends;
use caelumora_model::session::Session;
use caelumora_model::removal::Deletable;
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::{ComponentStore, EntityStore, EntityStoreFactory, EntityStores, StateStore, StoreBackend, StoreResult};
use caelumora_model::type_map::{TypeKey, TypeMap};
use crate::plugin::Plugin;
use crate::seed::{self, SeedSection};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::{Arc, OnceLock, Weak};
use crate::agent::manager::AgentManager;
use crate::group::GroupManager;
use crate::terminal::TerminalProvider;
use crate::scoped::{ScopeMember, ScopedEntities, Unowned};
use crate::workspace::WorkspaceManager;
use crate::workspace::reaper::CommandReaper;

#[derive(Debug)]
pub struct RuntimeManager {
    security_manager: Arc<dyn SecurityManager>,
    /// What may flow where, and who is cleared for what (issue 304): asked
    /// beside the `SecurityManager`, never by it.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    state_store: Arc<dyn StateStore>,
    entity_stores: EntityStores,
    /// The preconfigured components, which the seed puts in.
    component_store: Arc<dyn ComponentStore>,
    secret_manager: Arc<SecretManager>,
    /// The one writer of user records, asked at a login (issue 306).
    user_manager: Arc<UserManager>,
    component_registry: Arc<ComponentRegistry>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
    commands: ComponentConfig,
    /// What the system restricts of tool calls everywhere
    /// (`AppConfig::approvals`, issue 13).
    approval_rules: Arc<[ApprovalRule]>,
    /// One owner of every running agent's `AgentState` (issue 62).
    agent_states: Arc<AgentStates>,
    /// The one owner of workspaces and their places (issue 253).
    workspaces: Arc<WorkspaceManager>,
    /// What ends the commands nobody may run any longer (issues 254, 344).
    command_reaper: Arc<CommandReaper>,
    /// The one writer of teams (issue 286).
    groups: Arc<GroupManager>,
    /// The one owner of agents' lifecycle (issue 314).
    agents: Arc<AgentManager>,
    /// What deletes, restores and purges what lies in a scope with what
    /// holds it (issues 178, 319).
    scoped: Arc<ScopedEntities>,
    /// The services components may use (issue 296), each given to the
    /// kinds that declare it (`ComponentKind::uses`), and the views.
    services: Services,
    /// The sections of the seed the plugins claimed (issue 323).
    seed_sections: BTreeMap<String, Arc<dyn SeedSection>>,
}

/// What a channel runs with, made by its session (`SessionLoop::channel_runtime`) as the channel
/// is built: the process-wide services of the `RuntimeManager` - among them the controllers of its
/// users and agents (issue 121) - the user commands of its session (issue 28), the session's
/// controller (issue 13) and its loop (issue 361), and a handle to the channel itself. It is the
/// only source of `ActorRuntime`s, so every actor lives in exactly one channel and shares its
/// commands and handle.
#[derive(Debug, Clone)]
pub struct ChannelRuntime {
    runtime_manager: Arc<RuntimeManager>,
    command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
    /// The session the channel belongs to, live: what its agents' tool
    /// calls are guarded by (`ToolExecutor`). Held, not reached through
    /// `session_loop`: the guard must not lapse.
    session: Arc<SessionController>,
    /// The loop of that session, for the channel alone (`ChannelLoop::session_loop`) - never
    /// handed to the actors (issue 123). Weak, since the session owns its channels; dangling for a
    /// channel of no running session, in tests.
    session_loop: Weak<SessionLoop>,
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
    /// A runtime for a channel of the session `session` holds and `session_loop` runs, whose user
    /// commands are `command_executor`.
    pub fn new(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session: Arc<SessionController>, session_loop: Weak<SessionLoop>) -> Self {
        Self { runtime_manager, command_executor, session, session_loop, channel: ChannelHandle::default() }
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

    /// The controller of the channel's session.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> { &self.session }

    /// The loop of the channel's session, for the channel (`ChannelLoop::session_loop`); `None`
    /// once the session is gone, or for a channel of none, in tests.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> { self.session_loop.upgrade() }
}

/// What a channel is built with (`ChannelLoop::new`), each read once there into what the channel
/// holds or builds (issue 360): the runtime it is given offers the services by name rather than
/// handing out the `RuntimeManager` behind it (issue 191), and `ChannelLoop::new` takes no service
/// beside it.
impl ChannelRuntime {
    pub(crate) fn security_manager(&self) -> &Arc<dyn SecurityManager> { self.runtime_manager.security_manager() }

    pub(crate) fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { self.runtime_manager.confidentiality() }

    pub(crate) fn services(&self) -> &Services { self.runtime_manager.services() }

    pub(crate) fn component_registry(&self) -> &Arc<ComponentRegistry> { self.runtime_manager.component_registry() }

    pub(crate) fn state_store(&self) -> &Arc<dyn StateStore> { self.runtime_manager.state_store() }

    pub(crate) fn controller_manager(&self) -> &Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    /// The stores of the entities - for what a channel checks of its
    /// participants (issue 191).
    pub(crate) fn entity_stores(&self) -> &EntityStores { self.runtime_manager.entity_stores() }

    pub(crate) fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }
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
impl ChannelRuntime {
    /// A runtime for a channel of the session `session_id` that is not
    /// running - in tests: its session holds nothing but its id, and no
    /// rules, and has no loop.
    pub(crate) fn outside_a_session(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session_id: SessionId) -> Self {
        let session = runtime_manager.controller_manager().new_session_controller(crate::session::testing::bare_session(session_id));
        Self::new(runtime_manager, command_executor, session, Weak::new())
    }

    /// The runtime of the same channel built again, as after a restart -
    /// in tests: the same services, commands and session, and no channel
    /// attached yet.
    pub(crate) fn restarted(&self) -> Self {
        Self::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.command_executor), Arc::clone(&self.session), Weak::clone(&self.session_loop))
    }
}

/// The services of the runtime, by type (issues 296, 323): the policy, the
/// secrets, the managers of the core that components use and what the
/// plugins provide. A component is given those its kind uses
/// (`ComponentKind::uses`), the views and a plugin's routes reach them all.
pub type Services = TypeMap;

/// What builds the `RuntimeManager` (issue 323): the core's services and
/// managers, made first - their backends by `Backends`, the core's and the
/// plugins' (issue 324) -, and what the plugins register with it
/// (`Plugin::register`) - their component kinds, the stores of their kinds,
/// the services they provide, the members of a scope, their sections of
/// the configuration and of the seed. `build` makes the runtime of it.
pub struct RuntimeBuilder {
    backend: StoreBackend,
    state_store: Arc<dyn StateStore>,
    entity_stores: EntityStores,
    secret_manager: Arc<SecretManager>,
    user_manager: Arc<UserManager>,
    security_manager: Arc<dyn SecurityManager>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    component_registry: ComponentRegistry,
    component_defaults: ComponentConfig,
    commands: ComponentConfig,
    approval_rules: Arc<[ApprovalRule]>,
    event_bus: Arc<EventBus>,
    controller_manager: Arc<ControllerManager>,
    agent_states: Arc<AgentStates>,
    workspaces: Arc<WorkspaceManager>,
    command_reaper: Arc<CommandReaper>,
    groups: Arc<GroupManager>,
    agents: Arc<AgentManager>,
    scope_members: Vec<Arc<dyn ScopeMember>>,
    services: Services,
    /// The backends of the core and of the plugins, and what a backend may
    /// ask for - which a plugin's service is made of, too (issue 328).
    backends: Backends,
    backend_services: Services,
    /// The sections of the configuration that no plugin claimed yet.
    sections: BTreeMap<String, serde_json::Value>,
    /// The names of the sections of the configuration that were claimed.
    claimed: BTreeSet<String>,
    seed_sections: BTreeMap<String, Arc<dyn SeedSection>>,
}

impl fmt::Debug for RuntimeBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeBuilder").field("services", &self.services.names()).finish_non_exhaustive()
    }
}

impl RuntimeBuilder {
    /// The builder of the runtime everything in a process shares, with the
    /// core's services and managers made. `security_manager` is the one the
    /// whole process was configured with: it decides tool calls and what may
    /// be done to a channel, so it must be the same instance the stores were
    /// built with, not a second one made up here; so is `confidentiality`,
    /// which decides what may flow where (issue 304). `backend` keeps the
    /// entities of every kind, the core's and the plugins'. Of `config`, the
    /// runtime takes what it runs by itself, so that a binary cannot drop a
    /// configured part for a default: the global user commands (`commands`),
    /// the components that replace built-in defaults of facets of which an
    /// agent has one (`component_defaults`, issue 99), what the system
    /// restricts of tool calls (`approvals`, issue 13), the storages
    /// workspaces are made in (`workspaces`, issue 253), the user providers
    /// (`users`) and the sections of the plugins. `component_registry` holds the
    /// component kinds agents and sessions are built from -
    /// `ComponentRegistry::built_in`, to which a test may add stand-ins,
    /// such as a model of its own (issue 130). `backends` makes the
    /// storages and the user providers `config` names (issue 324), and
    /// later the backends of a plugin's service (`make_named`):
    /// `Backends::with_plugins` of the plugins registered afterwards.
    ///
    /// # Errors
    /// If a store of the core's kinds cannot be opened, or a storage of
    /// workspaces or a user provider cannot be made.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state_store: Arc<dyn StateStore>,
        backend: impl Into<StoreBackend>,
        secret_manager: Arc<SecretManager>,
        security_manager: Arc<dyn SecurityManager>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        config: &AppConfig,
        component_registry: ComponentRegistry,
        backends: Backends,
    ) -> Result<Self> {
        let backend = backend.into();
        let entity_stores = backend.entity_stores()?;
        let event_bus = Arc::new(EventBus::new(Arc::clone(&security_manager), Arc::clone(&confidentiality)));
        // What a backend may ask for: the policies, the secrets and, for a
        // user provider, the records of the users.
        let backend_services = Services::empty()
            .with(Arc::clone(&security_manager))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&entity_stores.users));
        let storages = Arc::new(backends.make_named::<dyn TerminalProvider>("storage of workspaces", &config.workspaces.storage, &backend_services)?);
        let controller_manager = Arc::new(ControllerManager::new(entity_stores.clone(), Arc::clone(&event_bus), Arc::clone(&security_manager), Arc::clone(&confidentiality), Arc::clone(&storages)));
        let agent_states = Arc::new(AgentStates::new(Arc::clone(&state_store), Arc::<EventBus>::clone(&event_bus)));
        let command_reaper = CommandReaper::new(&entity_stores, &event_bus, Arc::clone(&controller_manager), Arc::clone(&storages))?;
        let workspaces = Arc::new(WorkspaceManager::new(&entity_stores, Arc::clone(&event_bus), Arc::clone(&security_manager), Arc::clone(&confidentiality), storages, &config.workspaces));
        // Users are recorded and their changes announced on the event bus
        // (issue 306).
        let user_providers = backends.make_chain::<dyn UserProvider>("user provider", &config.users.providers, &backend_services)?;
        let user_manager = Arc::new(UserManager::new(user_providers, Arc::clone(&entity_stores.users), Arc::<EventBus>::clone(&event_bus)));
        let groups = Arc::new(GroupManager::new(&entity_stores, Arc::clone(&event_bus), Arc::clone(&security_manager)));
        let agents = Arc::new(AgentManager::new(&entity_stores, Arc::clone(&state_store), Arc::clone(&event_bus), Arc::clone(&security_manager)));
        let scope_members: Vec<Arc<dyn ScopeMember>> = vec![
            Arc::new(Unowned(Arc::clone(&entity_stores.personalities))),
            Arc::new(Unowned(Arc::clone(&entity_stores.session_templates))),
            Arc::<AgentManager>::clone(&agents),
            Arc::<WorkspaceManager>::clone(&workspaces),
        ];
        let services = Services::empty()
            .with(Arc::clone(&security_manager))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&workspaces));
        Ok(Self {
            backend,
            state_store,
            entity_stores,
            secret_manager,
            user_manager,
            security_manager,
            confidentiality,
            component_registry,
            component_defaults: config.component_defaults.clone(),
            commands: config.commands.clone(),
            approval_rules: config.approvals.rules.clone().into(),
            event_bus,
            controller_manager,
            agent_states,
            workspaces,
            command_reaper,
            groups,
            agents,
            scope_members,
            services,
            backends,
            backend_services,
            sections: config.plugins.clone(),
            claimed: BTreeSet::new(),
            seed_sections: BTreeMap::new(),
        })
    }

    /// The builder of the runtime as `config` describes it, with `plugins`
    /// registered: the policy, learning who belongs to a group from the
    /// store it guards (issue 23), the policy of what may flow where (issue
    /// 304), the stores, the secrets, the built-in component kinds and the
    /// backends of the core and of the plugins (issue 324) - first the
    /// plugins' backends, then what is made of them, then the rest of what
    /// the plugins bring.
    ///
    /// # Errors
    /// If a store cannot be opened, a secret provider cannot be made, or as
    /// `new` and `plugin`.
    pub fn from_config(config: &AppConfig, plugins: &[&dyn Plugin]) -> Result<Self> {
        let backends = Backends::with_plugins(plugins);
        // The policy is made before the store it guards, and bound to it
        // once it exists.
        let memberships = Arc::new(StoreMemberships::new());
        let security_manager = config.create_security_manager(Arc::<StoreMemberships>::clone(&memberships));
        let confidentiality = config.create_confidentiality_policy();
        let state_store = config.create_state_store(Arc::clone(&security_manager), Arc::clone(&confidentiality))?;
        let backend = config.open_entity_store(Arc::clone(&security_manager), Arc::clone(&confidentiality))?;
        memberships.bind(&backend.entity_stores()?.groups);
        let secret_services = Services::empty().with(Arc::clone(&security_manager)).with(Arc::clone(&confidentiality));
        let secret_providers = backends.make_chain::<dyn SecretProvider>("secret provider", &config.secrets.providers, &secret_services)?;
        let secret_manager = Arc::new(SecretManager::new(secret_providers, Arc::clone(&security_manager)));
        let registry = ComponentRegistry::built_in(backend.component_store());
        let mut builder = Self::new(state_store, backend, secret_manager, security_manager, confidentiality, config, registry, backends)?;
        for plugin in plugins {
            builder.plugin(*plugin)?;
        }
        Ok(builder)
    }

    /// Registers what `plugin` brings.
    ///
    /// # Errors
    /// Whatever its registration fails with, naming the plugin.
    pub fn plugin(&mut self, plugin: &dyn Plugin) -> Result<()> {
        plugin.register(self).with_context(|| format!("Cannot register the plugin {}", plugin.name()))
    }

    /// The store of the entities of `T`, a plugin's kind - the same for
    /// whoever asks.
    ///
    /// # Errors
    /// `StoreError::Conflict` if another kind has its name; if it cannot be
    /// opened.
    pub fn entity_store<T: Deletable>(&self) -> StoreResult<Arc<dyn EntityStore<T>>> {
        self.backend.entity_store()
    }

    /// The stores of the core's kinds.
    pub fn entity_stores(&self) -> &EntityStores {
        &self.entity_stores
    }

    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> {
        &self.security_manager
    }

    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> {
        &self.confidentiality
    }

    pub fn secret_manager(&self) -> &Arc<SecretManager> {
        &self.secret_manager
    }

    pub fn controller_manager(&self) -> &Arc<ControllerManager> {
        &self.controller_manager
    }

    /// The component kinds, to register a plugin's own with.
    pub fn components(&mut self) -> &mut ComponentRegistry {
        &mut self.component_registry
    }

    /// Offers `service` - a plugin's manager, say - to the component kinds
    /// that use it, to the views and to the routes (`Services`).
    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) {
        self.services.insert(service);
    }

    /// The service `T`, if the core or a plugin registered before provides
    /// it.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.services.get::<T>()
    }

    /// The backends of `T` - a service of the plugin's own - that `configs`
    /// names, by their names, made as the core's are: by the factories the
    /// core and the plugins registered (`Plugin::backends`), given what a
    /// backend may ask for. `what` names one of them in messages - "storage
    /// of artifacts".
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_named<T: ?Sized + 'static>(&self, what: &str, configs: &BTreeMap<String, BackendConfig>) -> ComponentResult<BTreeMap<String, Arc<T>>> {
        self.backends.make_named(what, configs, &self.backend_services)
    }

    /// Makes `member` go with what holds the scope it lies in: deleted,
    /// restored and purged with it, after the core's kinds.
    pub fn scope_member(&mut self, member: Arc<dyn ScopeMember>) {
        self.scope_members.push(member);
    }

    /// Claims the section `name` of the configuration and reads it as `C`;
    /// left out, it is `C::default()`.
    ///
    /// # Errors
    /// If another plugin claimed it, or it is not a `C`.
    pub fn config<C: DeserializeOwned + Default>(&mut self, name: &str) -> Result<C> {
        if !self.claimed.insert(name.to_string()) {
            bail!("The section '{name}' of the configuration is claimed twice");
        }
        match self.sections.remove(name) {
            Some(section) => serde_json::from_value(section).with_context(|| format!("Cannot read the section '{name}' of the configuration")),
            None => Ok(C::default()),
        }
    }

    /// Claims the section `name` of the seed files: `section` seeds what
    /// it holds.
    ///
    /// # Errors
    /// If another plugin claimed it.
    pub fn seed_section(&mut self, name: &str, section: Arc<dyn SeedSection>) -> Result<()> {
        if self.seed_sections.insert(name.to_string(), section).is_some() {
            bail!("The section '{name}' of the seed is claimed twice");
        }
        Ok(())
    }

    /// The runtime, with everything registered.
    ///
    /// # Errors
    /// If a section of the configuration is claimed by no plugin; a
    /// `ComponentError` if `component_defaults` cannot replace a default
    /// (see `ComponentRegistry::set_defaults`) - the process does not start
    /// then.
    pub fn build(mut self) -> Result<RuntimeManager> {
        if let Some(name) = self.sections.keys().next() {
            bail!("The configuration has a section '{name}' that no plugin claims");
        }
        self.component_registry.set_defaults(&self.component_defaults)?;
        let scoped = Arc::new(ScopedEntities::new(self.scope_members));
        Ok(RuntimeManager {
            security_manager: self.security_manager,
            confidentiality: self.confidentiality,
            state_store: self.state_store,
            entity_stores: self.entity_stores,
            component_store: self.backend.component_store(),
            secret_manager: self.secret_manager,
            user_manager: self.user_manager,
            component_registry: Arc::new(self.component_registry),
            event_bus: self.event_bus,
            controller_manager: self.controller_manager,
            commands: self.commands,
            approval_rules: self.approval_rules,
            agent_states: self.agent_states,
            workspaces: self.workspaces,
            command_reaper: self.command_reaper,
            groups: self.groups,
            agents: self.agents,
            scoped,
            services: self.services,
            seed_sections: self.seed_sections,
        })
    }
}

impl RuntimeManager {
    /// The runtime of the core alone, without plugins: as `RuntimeBuilder`
    /// builds it from the same parts, with the core's backends.
    ///
    /// # Errors
    /// As `RuntimeBuilder::new` and `RuntimeBuilder::build`.
    pub fn new(
        state_store: Arc<dyn StateStore>,
        backend: impl Into<StoreBackend>,
        secret_manager: Arc<SecretManager>,
        security_manager: Arc<dyn SecurityManager>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        config: &AppConfig,
        component_registry: ComponentRegistry,
    ) -> Result<Self> {
        RuntimeBuilder::new(state_store, backend, secret_manager, security_manager, confidentiality, config, component_registry, Backends::built_in())?.build()
    }

    /// Puts what the seed files `paths` hold into the stores (issue 105):
    /// the core's sections and those the plugins claimed.
    ///
    /// # Errors
    /// As `seed::load`.
    pub async fn seed(&self, paths: &[PathBuf]) -> Result<()> {
        seed::load(&self.entity_stores, self.component_store.as_ref(), self.confidentiality.as_ref(), &self.seed_sections, paths).await
    }

    /// The executor for the user commands of `session`: the global ones and
    /// the session's own (issue 28), built once for all of its channels, in
    /// the name of the session's owner (`ControllerManager::owner_context`,
    /// issue 321).
    ///
    /// # Errors
    /// A `ComponentError` if a command component cannot be built or two
    /// provide the same command - the session does not start then; whatever
    /// the store reports reading its owner.
    pub async fn new_command_executor(&self, session: &Session) -> Result<CommandExecutor<dyn ChannelCommandProvider>> {
        let security = self.controller_manager.owner_context(session.security_attributes.owner_id, session.id, ChannelId::default()).await?;
        let context = ComponentContext::new(security, self.services().clone());
        let mut providers = Vec::new();
        // Each configuration on its own, so that component ids only need to
        // be unique within one of them.
        for config in [&self.commands, &session.commands] {
            let container = ComponentContainer::from_config(config, &[TypeKey::of::<dyn ChannelCommandProvider>()], &self.component_registry, &context).await?;
            providers.extend(container.get_all::<dyn ChannelCommandProvider>(None)?);
        }
        Ok(CommandExecutor::new(Arc::clone(self.security_manager()), providers)?)
    }

    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    pub fn security_manager(&self) -> &Arc<dyn SecurityManager> { &self.security_manager }

    /// What may flow where, and who is cleared for what (issue 304).
    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { &self.confidentiality }

    pub fn state_store(&self) -> &Arc<dyn StateStore> { &self.state_store }

    /// The stores of the entities, one per kind (issue 323).
    pub fn entity_stores(&self) -> &EntityStores { &self.entity_stores }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { &self.secret_manager }

    /// The one writer of user records, which authenticates a login against
    /// the configured user providers (issues 306, 324).
    pub fn user_manager(&self) -> &Arc<UserManager> { &self.user_manager }

    pub fn services(&self) -> &Services { &self.services }

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
    pub fn agent_states(&self) -> &Arc<AgentStates> {
        &self.agent_states
    }

    /// The one owner of workspaces and their places (issue 253).
    pub fn workspace_manager(&self) -> &Arc<WorkspaceManager> {
        &self.workspaces
    }

    /// What ends the commands nobody may run any longer (issues 254, 344).
    pub(crate) fn command_reaper(&self) -> &Arc<CommandReaper> {
        &self.command_reaper
    }


    /// The one writer of teams (issue 286), and what deletes and restores
    /// the groups of the participants with their session or channel (issue
    /// 348).
    pub fn group_manager(&self) -> &Arc<GroupManager> {
        &self.groups
    }

    /// The one owner of agents' lifecycle (issue 314).
    pub fn agent_manager(&self) -> &Arc<AgentManager> {
        &self.agents
    }

    /// What deletes, restores and purges what lies in a scope with what
    /// holds it (issues 178, 319).
    pub(crate) fn scoped_entities(&self) -> &Arc<ScopedEntities> {
        &self.scoped
    }

    pub fn new_recorder(&self) -> Result<Box<dyn Recorder>> {
        Ok(Box::new(BlackholeRecorder::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use crate::testing::test_registry;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::user_provider::Credential;

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

    #[test]
    fn the_configuration_may_replace_the_context_provider_but_only_with_one() {
        let manager = |defaults: serde_json::Value| {
            let store = Arc::new(MemoryStore::new());
            let secrets = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default()), &AppConfig { component_defaults: defaults, ..Default::default() }, test_registry(Arc::<MemoryStore>::clone(&store)))
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!([{"id": "context", "kind": "turn_context_provider"}])).is_ok());
        let refused = manager(serde_json::json!([{"id": "context", "kind": "calculator_tool"}])).expect_err("provides no context");
        assert!(refused.to_string().contains("exactly one facet"), "{refused}");
    }

    #[tokio::test]
    async fn the_backends_the_configuration_names_are_made_when_the_runtime_is_built() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!("secrets:\n  providers: []\nworkspaces:\n  storage:\n    main:\n      type: local\n      root: \"{}\"\nusers:\n  providers:\n    - type: accept_any\n", dir.path().display());
        let runtime = RuntimeBuilder::from_config(&AppConfig::from_yaml(&yaml).unwrap(), &[]).unwrap().build().unwrap();
        assert_eq!(runtime.workspace_manager().storages().collect::<Vec<_>>(), [("main", false)]);
        let alice = runtime.user_manager().authenticate("alice", &Credential::Password("x")).await.expect("accepted by accept_any");
        assert_eq!(alice.username, "alice");

        for (yaml, names) in [
            ("workspaces:\n  storage:\n    main:\n      type: nfs\n", ["the storage of workspaces 'main' (nfs)", "known are: local"]),
            ("workspaces:\n  storage:\n    dev:\n      type: local\n      root: x\n      max_run_secs: 0\n", ["'dev'", "max_run_secs"]),
            ("users:\n  providers:\n    - type: accept_any\n      default_roles: [staff]\n", ["the user provider 1 (accept_any)", "staff"]),
            ("secrets:\n  providers:\n    - type: env\n    - type: vault\n", ["the secret provider 2 (vault)", "known are: env, file"]),
        ] {
            let refused = RuntimeBuilder::from_config(&AppConfig::from_yaml(yaml).unwrap(), &[]).map(|_| ()).expect_err(yaml);
            let message = format!("{refused:#}");
            assert!(names.iter().all(|name| message.contains(name)), "{yaml:?} gave: {message}");
        }
    }
}
