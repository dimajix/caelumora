use crate::actor::AgentKinds;
use crate::channel::Participation;
use crate::component::{ComponentRegistry, ComponentResult};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use anyhow::{bail, Context, Result};
use caelumora_model::approval::ApprovalRule;
use crate::agent::state::AgentStates;
use caelumora_model::component::ComponentConfig;
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::secret::{SecretManager, SecretProvider};
use caelumora_model::security::AccessPolicy;
use caelumora_model::user_provider::{UserManager, UserProvider};
use crate::backend::Backends;
use caelumora_model::removal::Deletable;
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::{EntityStore, EntityStoreFactory, EntityStores, StateStore, StoreBackend, StoreResult};
use crate::plugin::Plugin;
use crate::seed::SeedSection;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::group::GroupManager;
use crate::terminal::TerminalProvider;
use crate::scoped::{ScopeMember, ScopedEntities, Unowned};
use crate::workspace::WorkspaceManager;
use crate::workspace::reaper::CommandReaper;
use super::{RuntimeManager, Services};
use serde_json::Value;

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
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    component_registry: ComponentRegistry,
    /// The kinds of agent the program runs (issue 370).
    agent_kinds: AgentKinds,
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
    sections: BTreeMap<String, Value>,
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
    /// core's services and managers made. `access_policy` is the one the
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
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        config: &AppConfig,
        component_registry: ComponentRegistry,
        backends: Backends,
    ) -> Result<Self> {
        let backend = backend.into();
        let entity_stores = backend.entity_stores()?;
        let event_bus = Arc::new(EventBus::new(Arc::clone(&access_policy), Arc::clone(&confidentiality)));
        // What a backend may ask for: the policies, the secrets and, for a
        // user provider, the records of the users.
        let backend_services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&entity_stores.users));
        let storages = Arc::new(backends.make_named::<dyn TerminalProvider>("storage of workspaces", &config.workspaces.storage, &backend_services)?);
        let controller_manager = Arc::new(ControllerManager::new(entity_stores.clone(), Arc::clone(&event_bus), Arc::clone(&access_policy), Arc::clone(&confidentiality), Arc::clone(&storages)));
        let agent_states = Arc::new(AgentStates::new(Arc::clone(&state_store), Arc::<EventBus>::clone(&event_bus)));
        let command_reaper = CommandReaper::new(&entity_stores, &event_bus, Arc::clone(&controller_manager), Arc::clone(&storages))?;
        let workspaces = Arc::new(WorkspaceManager::new(&entity_stores, Arc::clone(&event_bus), Arc::clone(&access_policy), Arc::clone(&confidentiality), storages, &config.workspaces));
        // Users are recorded and their changes announced on the event bus
        // (issue 306).
        let user_providers = backends.make_chain::<dyn UserProvider>("user provider", &config.users.providers, &backend_services)?;
        let user_manager = Arc::new(UserManager::new(user_providers, Arc::clone(&entity_stores.users), Arc::<EventBus>::clone(&event_bus)));
        let groups = Arc::new(GroupManager::new(&entity_stores, Arc::clone(&event_bus), Arc::clone(&access_policy)));
        let agents = Arc::new(AgentManager::new(&entity_stores, Arc::clone(&state_store), Arc::clone(&event_bus), Arc::clone(&access_policy)));
        let scope_members: Vec<Arc<dyn ScopeMember>> = vec![
            Arc::new(Unowned(Arc::clone(&entity_stores.personalities))),
            Arc::new(Unowned(Arc::clone(&entity_stores.session_templates))),
            Arc::<AgentManager>::clone(&agents),
            Arc::<WorkspaceManager>::clone(&workspaces),
        ];
        let services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&workspaces));
        Ok(Self {
            backend,
            state_store,
            entity_stores,
            secret_manager,
            user_manager,
            access_policy,
            confidentiality,
            component_registry,
            agent_kinds: AgentKinds::default(),
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
        let access_policy = config.create_access_policy(Arc::<StoreMemberships>::clone(&memberships));
        let confidentiality = config.create_confidentiality_policy();
        let state_store = config.create_state_store(Arc::clone(&access_policy), Arc::clone(&confidentiality))?;
        let backend = config.open_entity_store(Arc::clone(&access_policy), Arc::clone(&confidentiality))?;
        memberships.bind(&backend.entity_stores()?.groups);
        let secret_services = Services::empty().with(Arc::clone(&access_policy)).with(Arc::clone(&confidentiality));
        let secret_providers = backends.make_chain::<dyn SecretProvider>("secret provider", &config.secrets.providers, &secret_services)?;
        let secret_manager = Arc::new(SecretManager::new(secret_providers, Arc::clone(&access_policy)));
        let registry = ComponentRegistry::built_in(backend.component_store());
        let mut builder = Self::new(state_store, backend, secret_manager, access_policy, confidentiality, config, registry, backends)?;
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

    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        &self.access_policy
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

    /// The kinds of agent the program runs (issue 370), none to begin
    /// with, to register a plugin's with - the agents'
    /// (`caelumora_agents::plugin::AgentsPlugin`).
    pub fn agent_kinds(&mut self) -> &mut AgentKinds {
        &mut self.agent_kinds
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
        let participation = Arc::new(Participation::new(Arc::clone(&self.entity_stores.agents), Arc::clone(&self.controller_manager), Arc::clone(&self.access_policy), Arc::clone(&self.confidentiality)));
        Ok(RuntimeManager {
            access_policy: self.access_policy,
            confidentiality: self.confidentiality,
            state_store: self.state_store,
            entity_stores: self.entity_stores,
            component_store: self.backend.component_store(),
            secret_manager: self.secret_manager,
            user_manager: self.user_manager,
            component_registry: Arc::new(self.component_registry),
            agent_kinds: Arc::new(self.agent_kinds),
            event_bus: self.event_bus,
            controller_manager: self.controller_manager,
            participation,
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::user_provider::Credential;

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
