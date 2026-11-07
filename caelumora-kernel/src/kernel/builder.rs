//! `KernelBuilder`: what makes the `Kernel` of a process - its own services and managers first,
//! then what each plugin registers (issue 323), then what goes with both: the `AgentManager` with
//! what the plugins keep behind an agent, and the members of a scope.

use super::extension::Extensions;
use super::Kernel;
use crate::actor::ActorManager;
use crate::actor::AgentKinds;
use crate::agent::state::AgentStates;
use crate::agent::{AgentManager, AgentMember};
use crate::attachment::AttachmentManager;
use crate::backend::{Backends, NamedBackends};
use crate::channel::{ChannelLoop, Participation};
use crate::command::CommandExecutor;
use crate::component::{ComponentRegistry, Origin};
use crate::content::ContentStorage;
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::group::GroupManager;
use crate::library::ForLibrary;
use crate::plugin::Plugin;
use crate::scoped::{ScopeHolders, ScopeMember, ScopedEntities, ScopedKind, Unowned};
use crate::seed::SeedSection;
use crate::terminal::TerminalProvider;
use crate::user_provider::{UserManager, UserProvider};
use crate::view::SessionView;
use crate::workspace::reaper::CommandReaper;
use crate::workspace::{UsableWorkspace, WorkspaceManager, WorkspaceUse};
use anyhow::{bail, Context, Result};
use caelumora_api::component::Face;
use caelumora_api::component::{ComponentResult, Services};
use caelumora_api::library::Library;
use caelumora_api::workspace::{WorkspaceReader, WorkspaceWriter};
use caelumora_domain::approval::ApprovalRule;
use caelumora_domain::component::ComponentConfig;
use caelumora_domain::config::{AppConfig, BackendConfig};
use caelumora_domain::prompt_catalogue::{
    PromptCatalogue, DEFAULT_VARIANT, KERNEL, KERNEL_PROMPTS,
};
use caelumora_domain::store::{StoreResult, Stored};
use caelumora_domain::type_map::{ErasedArc, TypeKey};
use caelumora_security::confidentiality::ConfidentialityPolicy;
use caelumora_security::secret::{SecretManager, SecretProvider};
use caelumora_security::{AccessPolicy, Policies};
use caelumora_store::memberships::StoreMemberships;
use caelumora_store::{EntityStore, EntityStores, Persistence, PersistenceBackend};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

/// What builds the `Kernel` (issue 323): the kernel's services and managers, made first - their
/// backends by `Backends`, the kernel's and the plugins' (issue 324) -, what the plugins register
/// with it (`Plugin::register`) - their component kinds, the stores of their kinds, the services
/// they provide, the members of a scope, what they keep behind an agent, their sections of the
/// configuration and of the seed -, and what the libraries register through a `LibraryBuilder`
/// (`Library::register`, issue 516). `build` makes the kernel of it, with what goes with what the
/// plugins registered: the `AgentManager` and the members of a scope, the kernel's before the
/// plugins'.
pub struct KernelBuilder {
    persistence: Persistence,
    entity_stores: EntityStores,
    secret_manager: Arc<SecretManager>,
    user_manager: Arc<UserManager>,
    policies: Policies,
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
    /// The storages of content (issue 282), which their owners - a
    /// plugin's manager - are made with.
    content_storages: Arc<NamedBackends<dyn ContentStorage>>,
    /// The backends the kernel and the plugins registered, which make what a plugin's service is
    /// served by (`make_named`, issue 520), with the services they may ask for.
    backends: Backends,
    backend_services: Services,
    attachments: Arc<AttachmentManager>,
    groups: Arc<GroupManager>,
    /// What the plugins keep behind an agent (issue 518), which its manager - made with the kernel,
    /// once they registered - removes as it purges an agent.
    agent_members: Vec<Arc<dyn AgentMember>>,
    /// The plugins' members of a scope, gone through after the kernel's.
    scope_members: Vec<Arc<dyn ScopedKind>>,
    scope_holders: Arc<ScopeHolders>,
    services: Services,
    /// The services the libraries provide (`LibraryBuilder::provide`), which a library's kinds may
    /// use beside the texts (issue 516).
    services_of_libraries: Vec<TypeKey>,
    /// What the plugins and the kernel bind to an agent's work (issue 516).
    extensions: Extensions,
    /// The texts given to models, to which the plugins and the libraries add theirs (issue 57); a
    /// service once the kernel is built.
    prompts: PromptCatalogue,
    /// The variants of the texts the configuration names, read once every plugin and library added
    /// its texts.
    prompt_variants: BTreeMap<String, PathBuf>,
    /// The sections of the plugins in the configuration (`plugins`, issue 323).
    plugin_sections: Sections,
    /// The sections of the libraries in the configuration (`libraries`, issue 516).
    library_sections: Sections,
    seed_sections: BTreeMap<String, Arc<dyn SeedSection>>,
}

/// The sections of the configuration of one kind of what joins the kernel - the plugins' or the
/// libraries' (issues 323, 516): each claimed by one of them, and read as it claims it.
struct Sections {
    /// The key of the configuration they lie under.
    key: &'static str,
    /// What claims them, as messages name it.
    claimant: &'static str,
    /// Those not claimed yet.
    unclaimed: BTreeMap<String, Value>,
    /// The names of those claimed.
    claimed: BTreeSet<String>,
}

impl Sections {
    fn new(key: &'static str, claimant: &'static str, sections: &BTreeMap<String, Value>) -> Self {
        Self { key, claimant, unclaimed: sections.clone(), claimed: BTreeSet::new() }
    }

    /// Claims the section `name` and reads it as `C`; left out, it is `C::default()`.
    ///
    /// # Errors
    /// If it was claimed before, or it is not a `C`.
    fn claim<C: DeserializeOwned + Default>(&mut self, name: &str) -> Result<C> {
        let key = self.key;
        match self.claim_value(name)? {
            Some(section) => serde_json::from_value(section).with_context(|| {
                format!("Cannot read the section '{key}.{name}' of the configuration")
            }),
            None => Ok(C::default()),
        }
    }

    /// Claims the section `name`, as it stands; `None` if it is left out.
    ///
    /// # Errors
    /// If it was claimed before.
    fn claim_value(&mut self, name: &str) -> Result<Option<Value>> {
        let key = self.key;
        if !self.claimed.insert(name.to_string()) {
            bail!("The section '{key}.{name}' of the configuration is claimed twice");
        }
        Ok(self.unclaimed.remove(name))
    }

    /// Whether every section was claimed.
    ///
    /// # Errors
    /// Naming a section none claimed.
    fn all_claimed(&self) -> Result<()> {
        if let Some(name) = self.unclaimed.keys().next() {
            bail!(
                "The configuration has a section '{}.{name}' that no {} claims",
                self.key,
                self.claimant
            );
        }
        Ok(())
    }
}

impl fmt::Debug for KernelBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelBuilder")
            .field("services", &self.services.names())
            .finish_non_exhaustive()
    }
}

impl KernelBuilder {
    /// The builder of the kernel everything in a process shares, with the kernel's services and
    /// managers made. `policies` are those the whole process was configured with: the access policy
    /// decides tool calls and what may be done to a channel, the confidentiality policy what may
    /// flow where (issue 304). `persistence` keeps the entities of every kind, the kernel's and the
    /// plugins', and the states; the builder opens its stores by `policies`, so that they check by
    /// nothing else (issue 449). Of `config`, the kernel takes what it runs by itself, so that a
    /// binary cannot drop a configured part for a default: the global user commands (`commands`),
    /// the components that replace built-in defaults of facets of which an agent has one
    /// (`component_defaults`, issue 99), what the system restricts of tool calls (`approvals`,
    /// issue 13), the storages workspaces are made in (`workspaces`, issue 253), the storages of
    /// content (`content`, issue 282), what a message may carry (`attachments`, issue 282), the
    /// user providers (`users`) and the sections of the plugins and the libraries. The component
    /// kinds agents and sessions are built from are the built-in ones
    /// (`ComponentRegistry::built_in`), resolving the bundles in the builder's stores; a plugin
    /// adds its own, and a test stand-ins, such as a model of its own (issue 130), through
    /// `components`. `backends` makes the storages and the user providers `config` names (issue
    /// 324), and what the services of the plugins are served by (`make_named`):
    /// `Backends::with_plugins` of the plugins registered afterwards.
    ///
    /// # Errors
    /// If a store of the kernel's kinds cannot be opened, or a storage of workspaces or of content
    /// or a user provider cannot be made.
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    pub fn new(
        persistence: Persistence,
        secret_manager: Arc<SecretManager>,
        policies: Policies,
        config: &AppConfig,
        backends: Backends,
    ) -> Result<Self> {
        let access_policy = Arc::clone(policies.access_policy());
        let confidentiality = Arc::clone(policies.confidentiality());
        let state_store = persistence.state_store();
        let entity_stores = EntityStores::open(&persistence, &policies)?;
        let scope_holders = Arc::new(ScopeHolders::new(&entity_stores));
        let event_bus = Arc::new(EventBus::new(&policies));
        // What a backend may ask for: the policies, the secrets and, for a
        // user provider, the records of the users.
        let backend_services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&entity_stores.users));
        let storages = Arc::new(backends.make_named::<dyn TerminalProvider>(
            "storage of workspaces",
            &config.workspaces.storage,
            &backend_services,
        )?);
        let controller_manager = Arc::new(ControllerManager::new(
            entity_stores.clone(),
            Arc::clone(&event_bus),
            &policies,
            Arc::clone(&storages),
            Arc::clone(&state_store),
            persistence.history_store(),
        ));
        let agent_states = Arc::new(AgentStates::new(
            Arc::clone(&state_store),
            Arc::<EventBus>::clone(&event_bus),
        ));
        let command_reaper = CommandReaper::new(
            &entity_stores,
            &event_bus,
            Arc::clone(&controller_manager),
            Arc::clone(&storages),
        )?;
        let workspaces = Arc::new(WorkspaceManager::new(
            &entity_stores,
            Arc::clone(&scope_holders),
            Arc::clone(&event_bus),
            &policies,
            storages,
        ));
        let content_storages = Arc::new(backends.make_named::<dyn ContentStorage>(
            "storage of content",
            &config.content.storage,
            &backend_services,
        )?);
        let attachments = Arc::new(AttachmentManager::new(
            Arc::clone(&confidentiality),
            Arc::clone(&content_storages),
            &config.attachments,
        )?);
        // Users are recorded and their changes announced on the event bus
        // (issue 306).
        let user_providers = backends.make_chain::<dyn UserProvider>(
            "user provider",
            &config.users.providers,
            &backend_services,
        )?;
        let user_manager = Arc::new(UserManager::new(
            user_providers,
            Arc::clone(&entity_stores.users),
            Arc::<EventBus>::clone(&event_bus),
        ));
        let groups = Arc::new(GroupManager::new(
            &entity_stores,
            Arc::clone(&event_bus),
            Arc::clone(&access_policy),
        ));
        let services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&workspaces))
            .with(Arc::clone(&attachments));
        // The kernel's texts, before any plugin adds its own.
        let mut prompts = PromptCatalogue::new();
        prompts.add(KERNEL, KERNEL_PROMPTS)?;
        let component_registry =
            ComponentRegistry::built_in(Arc::clone(&entity_stores.component_bundles));
        let extensions = Self::kernel_extensions()?;
        Ok(Self {
            persistence,
            entity_stores,
            secret_manager,
            user_manager,
            policies,
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
            content_storages,
            backends,
            backend_services,
            attachments,
            groups,
            agent_members: Vec::new(),
            scope_members: Vec::new(),
            scope_holders,
            services,
            services_of_libraries: Vec::new(),
            extensions,
            prompts,
            prompt_variants: config.prompts.clone(),
            plugin_sections: Sections::new("plugins", "plugin", &config.plugins),
            library_sections: Sections::new("libraries", "library", &config.libraries),
            seed_sections: BTreeMap::new(),
        })
    }

    /// The kernel's own extensions (issue 516): the workspace a work may use in its channel, as a
    /// reader and a writer.
    fn kernel_extensions() -> Result<Extensions> {
        let mut extensions = Extensions::default();
        extensions.insert::<dyn WorkspaceReader, _, _>(Face::Reader, |session| async move {
            let usable = UsableWorkspace::of_work(&session).await?;
            Some(Arc::new(usable) as Arc<dyn WorkspaceReader>)
        })?;
        extensions.insert::<dyn WorkspaceWriter, _, _>(Face::Writer, |session| async move {
            WorkspaceUse::of_work(&session).map(|used| Arc::new(used) as Arc<dyn WorkspaceWriter>)
        })?;
        Ok(extensions)
    }

    /// The builder of the kernel as `config` describes it, with `plugins` registered: the policy,
    /// learning who belongs to a group from the store it guards (issue 23), the policy of what may
    /// flow where (issue 304), the secrets, the stores, the built-in component kinds and the
    /// backends of the kernel and of the plugins (issue 324) - first the plugins' backends, then
    /// what is made of them, then the rest of what the plugins bring. The secrets are made before
    /// the stores, whose backends may need one to connect, and the backends of the stores are
    /// opened last (`Persistence::open`, issue 447), once the plugins have opened the stores of
    /// their kinds.
    ///
    /// # Errors
    /// If a secret provider or a store cannot be made, a store cannot be
    /// opened, or as `new` and `plugin`.
    pub async fn from_config(config: &AppConfig, plugins: &[&dyn Plugin]) -> Result<Self> {
        let backends = Backends::with_plugins(plugins);
        // The policy is made before the store it guards, and bound to the kernel's once it exists.
        let memberships = Arc::new(StoreMemberships::new());
        let policies = Policies::configured(config, Arc::<StoreMemberships>::clone(&memberships));
        let access_policy = Arc::clone(policies.access_policy());
        let secret_services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(policies.confidentiality()));
        let secret_providers = backends.make_chain::<dyn SecretProvider>(
            "secret provider",
            &config.secrets.providers,
            &secret_services,
        )?;
        let secret_manager =
            Arc::new(SecretManager::new(secret_providers, Arc::clone(&access_policy)));
        // The stores, over the backends the configuration names (issue 446),
        // which may ask for the secrets (issue 447).
        let store_services = secret_services.with(Arc::clone(&secret_manager));
        let backend = |what: &str, name: &str, backend: &BackendConfig| {
            backends.make_one::<dyn PersistenceBackend>(what, name, backend, &store_services)
        };
        let states = backend("state store", "state_store", &config.persistence.state_store)?;
        let entities = backend("entity store", "entity_store", &config.persistence.entity_store)?;
        let persistence =
            Persistence::new(entities, states).context("Cannot make the state store")?;
        let mut builder =
            Self::new(persistence.clone(), secret_manager, policies, config, backends)?;
        memberships.bind(&builder.entity_stores().groups);
        for plugin in plugins {
            builder.plugin(*plugin)?;
        }
        persistence.open().await.context("Cannot open the stores")?;
        Ok(builder)
    }

    /// Registers what `plugin` brings, its component kinds as the plugin's.
    ///
    /// # Errors
    /// Whatever its registration fails with, naming the plugin.
    pub fn plugin(&mut self, plugin: &dyn Plugin) -> Result<()> {
        self.registering_as(Origin::Plugin(plugin.name()), |builder| {
            plugin
                .register(builder)
                .with_context(|| format!("Cannot register the plugin {}", plugin.name()))
        })
    }

    /// Registers what `library` brings, with the part of this builder a library may use
    /// (`LibraryBuilder`, issue 516), its component kinds as the library's.
    ///
    /// # Errors
    /// Whatever its registration fails with, naming the library.
    pub fn library(&mut self, library: &dyn Library) -> Result<()> {
        self.registering_as(Origin::Library(library.name()), |builder| {
            library
                .register(&mut ForLibrary(builder))
                .with_context(|| format!("Cannot register the library {}", library.name()))
        })
    }

    /// What `register` answers, the component kinds it registers and the singles it declares made
    /// `origin`'s (issue 521); what is registered after it is the kernel's again.
    pub(crate) fn registering_as<R>(
        &mut self,
        origin: Origin,
        register: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.component_registry.registering(origin);
        let registered = register(self);
        self.component_registry.registering(Origin::Kernel);
        registered
    }

    /// The store of the entities of `T`, a plugin's kind: over the one
    /// storage of the kind, checked by the builder's policies.
    ///
    /// # Errors
    /// `StoreError::Conflict` if another kind has its name; if it cannot be
    /// opened.
    pub fn entity_store<T: Stored>(&self) -> StoreResult<Arc<dyn EntityStore<T>>> {
        self.persistence.entity_store(&self.policies)
    }

    /// The stores of the kernel's kinds.
    pub fn entity_stores(&self) -> &EntityStores {
        &self.entity_stores
    }

    /// The holders of scopes, asked by whoever puts something into one (issue 458).
    pub fn scope_holders(&self) -> &Arc<ScopeHolders> {
        &self.scope_holders
    }

    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// The policies of the process, the pair its stores are checked by.
    pub fn policies(&self) -> &Policies {
        &self.policies
    }

    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        self.policies.access_policy()
    }

    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> {
        self.policies.confidentiality()
    }

    pub fn secret_manager(&self) -> &Arc<SecretManager> {
        &self.secret_manager
    }

    pub fn controller_manager(&self) -> &Arc<ControllerManager> {
        &self.controller_manager
    }

    /// The storages of content the configuration names (issue 282), to
    /// make the owner of what is kept there with.
    pub fn content_storages(&self) -> &Arc<NamedBackends<dyn ContentStorage>> {
        &self.content_storages
    }

    /// The backends of `T` that `configs` - a plugin's section - names, by their names, made by the
    /// factories the kernel and the plugins registered (`Plugin::backends`), as the kernel makes
    /// its own: for a plugin whose service is served by backends other plugins bring - the models
    /// (issue 520). `what` names one of them in messages.
    ///
    /// # Errors
    /// `ComponentError::Backend` naming the first that cannot be made.
    pub fn make_named<T: ?Sized + 'static>(
        &self,
        what: &str,
        configs: &BTreeMap<String, BackendConfig>,
    ) -> ComponentResult<NamedBackends<T>> {
        self.backends.make_named(what, configs, &self.backend_services)
    }

    /// The component kinds, to register a plugin's own with.
    pub fn components(&mut self) -> &mut ComponentRegistry {
        &mut self.component_registry
    }

    /// The kinds of agent the program runs (issue 370), none to begin with, to register a
    /// library's with (`LibraryBuilder::agent_kinds`) - the agents'
    /// (`caelumora_agents::library::AgentsLibrary`): kinds of agent are the agent side's.
    pub(crate) fn agent_kinds(&mut self) -> &mut AgentKinds {
        &mut self.agent_kinds
    }

    /// The texts given to models (issue 57), to add a plugin's with - under its namespace, from the
    /// file it compiles in. Offered to the component kinds that use it once the kernel is built.
    pub fn prompts(&mut self) -> &mut PromptCatalogue {
        &mut self.prompts
    }

    /// Offers `service` - a plugin's manager, say - to the component kinds
    /// that use it, to the views and to the routes (`Services`).
    ///
    /// # Errors
    /// If a service of its type is offered already - by the kernel, a plugin or a library: none
    /// replaces it (issue 521).
    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) -> Result<()> {
        self.offer(ErasedArc::of(service))
    }

    /// Offers `service`, a library's, its type forgotten, as `provide` offers a plugin's: one that
    /// the kinds of the libraries may use (`LibraryBuilder::provide_erased`, issue 516).
    ///
    /// # Errors
    /// As `provide`.
    pub(crate) fn provide_of_library(&mut self, service: ErasedArc) -> Result<()> {
        let key = service.key();
        self.offer(service)?;
        self.services_of_libraries.push(key);
        Ok(())
    }

    /// Offers `service` under the key of its type, unless a service of that type is offered
    /// already.
    fn offer(&mut self, service: ErasedArc) -> Result<()> {
        let key = service.key();
        if self.services.contains(key) {
            bail!("The service {} is offered already", key.name());
        }
        self.services.insert_erased(service);
        Ok(())
    }

    /// Binds the reader `T` to every piece of work of an agent (`ActorRuntime::reader`, issue 516):
    /// what of a plugin's service the work reads by, made by `bind` from the work's session as the
    /// work reads it - `None` where it is not offered -, so that it acts in the work's context.
    ///
    /// # Errors
    /// If `T` is bound already - by the kernel, or by a plugin registered before: no plugin
    /// replaces what another binds.
    pub fn extend_readers<T, F, Fut>(&mut self, bind: F) -> Result<()>
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(SessionView) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Arc<T>>> + Send + 'static,
    {
        self.extensions.insert(Face::Reader, bind)
    }

    /// Binds the writer `T` to every piece of work of an agent (`ActorRuntime::writer`): what of a
    /// plugin's service the work acts by, made as `extend_readers` makes a reader.
    ///
    /// # Errors
    /// As `extend_readers`.
    pub fn extend_writers<T, F, Fut>(&mut self, bind: F) -> Result<()>
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(SessionView) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Arc<T>>> + Send + 'static,
    {
        self.extensions.insert(Face::Writer, bind)
    }

    /// The service `T`, if the kernel or a plugin registered before provides it.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.services.get::<T>()
    }

    /// Makes `member` go with what holds the scope it lies in: deleted, restored and purged with
    /// it, after the kernel's kinds.
    pub fn scope_member<M: ScopeMember + 'static>(&mut self, member: Arc<M>) {
        self.scope_members.push(member);
    }

    /// Makes what `member` keeps behind an agent go with it (issue 518): removed when the agent is
    /// purged, after the kernel's own state of it.
    pub fn agent_member<M: AgentMember + 'static>(&mut self, member: Arc<M>) {
        self.agent_members.push(member);
    }

    /// Claims the section `name` of the plugins in the configuration (`plugins`) and reads it as
    /// `C`; left out, it is `C::default()`.
    ///
    /// # Errors
    /// If another plugin claimed it, or it is not a `C`.
    pub fn config<C: DeserializeOwned + Default>(&mut self, name: &str) -> Result<C> {
        self.plugin_sections.claim(name)
    }

    /// Claims the section `name` of the libraries in the configuration (`libraries`), as it stands,
    /// as `config` claims a plugin's (`LibraryBuilder::section`, issue 516).
    ///
    /// # Errors
    /// If another library claimed it.
    pub(crate) fn library_section(&mut self, name: &str) -> Result<Option<Value>> {
        self.library_sections.claim_value(name)
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

    /// Refuses what a library registered that reaches what is the kernel's (issue 516): a kind or a
    /// single of a facet the kernel builds in its own name - for a channel (`ChannelLoop`) or for
    /// the commands of a session (`CommandExecutor`) -, or a kind that uses a service that is
    /// neither a library's nor the texts: a library is trusted for nothing, and its components
    /// reach nothing of the kernel but through the interface they are given.
    ///
    /// # Errors
    /// As `ComponentRegistry::keeps_libraries_to_their_side`.
    fn libraries_keep_to_their_side(&self) -> Result<()> {
        let channel = ChannelLoop::COMPONENT_FACETS.iter();
        let asked: Vec<TypeKey> =
            channel.chain(CommandExecutor::COMPONENT_FACETS).copied().collect();
        let texts = TypeKey::of::<PromptCatalogue>();
        self.component_registry.keeps_libraries_to_their_side(&asked, |service| {
            service.id == texts.id || self.services_of_libraries.iter().any(|s| s.id == service.id)
        })
    }

    /// The kernel, with everything registered.
    ///
    /// # Errors
    /// If a section of the configuration is claimed by no plugin or library; if a component kind
    /// was registered under a name that was taken, or a single declared twice (issue 521); a
    /// `ComponentError` if `component_defaults` cannot replace a default
    /// (see `ComponentRegistry::set_defaults`) - the process does not start
    /// then.
    pub fn build(mut self) -> Result<Kernel> {
        self.plugin_sections.all_claimed()?;
        self.library_sections.all_claimed()?;
        self.component_registry.registered_once()?;
        let unbound = self
            .component_registry
            .extensions_declared()
            .find(|(_, face, key)| !self.extensions.binds(*face, *key));
        if let Some((kind, face, key)) = unbound {
            bail!(
                "The component kind '{kind}' needs the {face} {}, which no plugin binds",
                key.name()
            );
        }
        self.libraries_keep_to_their_side()?;
        self.component_registry.set_defaults(&self.component_defaults)?;
        // The default variant first: the others fall back to it.
        let (defaults, variants): (Vec<_>, Vec<_>) = self
            .prompt_variants
            .iter()
            .partition(|(variant, _)| variant.as_str() == DEFAULT_VARIANT);
        for (variant, dir) in defaults.into_iter().chain(variants) {
            self.prompts.add_variant(variant, dir)?;
        }
        let prompts = Arc::new(mem::take(&mut self.prompts));
        self.provide(Arc::clone(&prompts))?;
        let agents = Arc::new(AgentManager::new(
            &self.entity_stores,
            Arc::clone(&self.scope_holders),
            self.persistence.state_store(),
            self.agent_members,
            Arc::clone(&self.event_bus),
            Arc::clone(self.policies.access_policy()),
        ));
        let kernel_members: [Arc<dyn ScopedKind>; 5] = [
            Arc::new(Unowned(Arc::clone(&self.entity_stores.personalities))),
            Arc::new(Unowned(Arc::clone(&self.entity_stores.session_templates))),
            Arc::new(Unowned(Arc::clone(&self.entity_stores.component_bundles))),
            Arc::<AgentManager>::clone(&agents),
            Arc::<WorkspaceManager>::clone(&self.workspaces),
        ];
        let scope_members = kernel_members.into_iter().chain(self.scope_members).collect();
        let scoped = Arc::new(ScopedEntities::new(scope_members));
        let participation = Arc::new(Participation::new(
            Arc::clone(&self.entity_stores.agents),
            Arc::clone(&self.controller_manager),
            &self.policies,
        ));
        Ok(Kernel {
            policies: self.policies,
            state_store: self.persistence.state_store(),
            history_store: self.persistence.history_store(),
            transcript_store: self.persistence.transcript_store(),
            entity_stores: self.entity_stores,
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
            attachments: self.attachments,
            groups: self.groups,
            agents,
            actors: Arc::new(ActorManager::new()),
            scoped,
            services: self.services,
            extensions: self.extensions,
            prompts,
            seed_sections: self.seed_sections,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ChannelCommandProvider;
    use crate::moderator::{Moderator, ResponderPolicy};
    use crate::testing::{Script, ScriptedAgents, TestKernel};
    use crate::user_provider::Credential;
    use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
    use caelumora_api::library::LibraryBuilder;
    use caelumora_api::model::LanguageModelClient;
    use caelumora_domain::command::HELP_KIND;
    use caelumora_domain::prompt_catalogue::PromptVariant;
    use caelumora_security::AllowAllPolicy;
    use std::fs;

    #[tokio::test]
    async fn the_backends_the_configuration_names_are_made_when_the_kernel_is_built() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!("secrets:\n  providers: []\nworkspaces:\n  storage:\n    main:\n      type: local\n      root: \"{0}\"\ncontent:\n  storage:\n    kept:\n      type: local\n      root: \"{0}\"\nusers:\n  providers:\n    - type: accept_any\n", dir.path().display());
        let builder =
            KernelBuilder::from_config(&AppConfig::from_yaml(&yaml).unwrap(), &[]).await.unwrap();
        assert!(
            builder.content_storages().get("kept").is_some(),
            "for their owners to be made with"
        );
        let kernel = builder.build().unwrap();
        assert_eq!(kernel.workspace_manager().storages().collect::<Vec<_>>(), [("main", false)]);
        let alice = kernel
            .user_manager()
            .authenticate("alice", &Credential::Password("x"))
            .await
            .expect("accepted by accept_any");
        assert_eq!(alice.username, "alice");

        for (yaml, names) in [
            ("workspaces:\n  storage:\n    main:\n      type: nfs\n", ["the storage of workspaces 'main' (nfs)", "known are: local"]),
            ("workspaces:\n  storage:\n    dev:\n      type: local\n      root: x\n      max_run_secs: 0\n", ["'dev'", "max_run_secs"]),
            ("content:\n  storage:\n    kept:\n      type: s3\n", ["the storage of content 'kept' (s3)", "known are: local"]),
            ("content:\n  storage:\n    kept:\n      type: local\n", ["the storage of content 'kept' (local)", "root"]),
            ("users:\n  providers:\n    - type: accept_any\n      default_roles: [staff]\n", ["the user provider 1 (accept_any)", "staff"]),
            ("secrets:\n  providers:\n    - type: env\n    - type: vault\n", ["the secret provider 2 (vault)", "known are: env, file"]),
        ] {
            let refused = KernelBuilder::from_config(&AppConfig::from_yaml(yaml).unwrap(), &[]).await.map(|_| ()).expect_err(yaml);
            let message = format!("{refused:#}");
            assert!(names.iter().all(|name| message.contains(name)), "{yaml:?} gave: {message}");
        }
    }

    /// Issue 516: a component kind of a library keeps to the agent side - the kernel is not built
    /// where one, registered either way, provides a facet the kernel builds in its own name, also
    /// one the kernel's kinds need, or uses a service of the kernel but the texts, nor where a
    /// library declares a single of the kernel's or offers a service in place of one -, while the
    /// same kind, registered by the kernel or a plugin, may; and a library's own service is the
    /// libraries' kinds' to use.
    #[test]
    fn the_kernel_is_not_built_where_a_library_reaches_what_is_the_kernel_s() {
        /// A service a library offers its own kinds.
        #[derive(Debug)]
        struct OwnService;
        /// What a library does as it registers, besides offering its own service.
        struct Brings(fn(&mut LibraryComponents<'_>));
        impl Library for Brings {
            fn name(&self) -> &'static str {
                "brings"
            }
            fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
                library.provide(Arc::new(OwnService))?;
                (self.0)(&mut library.components());
                Ok(())
            }
        }
        /// Registers the kind "brought", as `declare` declares it.
        fn brought(
            components: &mut LibraryComponents<'_>,
            declare: fn(ComponentKind) -> ComponentKind,
        ) {
            components.register(declare(ComponentKind::new("brought")), |_, _| {
                Ok(ComponentFacets::empty())
            });
        }
        let built = |brings: &Brings| {
            TestKernel { libraries: vec![brings], ..Default::default() }.try_build().map(drop)
        };

        built(&Brings(|c| brought(c, |kind| kind.uses::<OwnService>().uses::<PromptCatalogue>())))
            .expect("its own service and the texts");
        built(&Brings(|c| brought(c, |kind| kind.provides::<dyn LanguageModelClient>())))
            .expect("a model of its own, the agent's affair (issue 520)");
        let refusing: [(Brings, &str); 6] = [
            (Brings(|c| brought(c, |kind| kind.provides::<dyn Moderator>())), "provides Moderator"),
            (
                Brings(|c| brought(c, |kind| kind.provides::<dyn ResponderPolicy>())),
                "provides ResponderPolicy",
            ),
            (
                Brings(|c| brought(c, |kind| kind.provides::<dyn ChannelCommandProvider>())),
                "provides ChannelCommandProvider",
            ),
            (Brings(|c| brought(c, |kind| kind.uses::<dyn AccessPolicy>())), "uses AccessPolicy"),
            (
                Brings(|c| {
                    let kind = ComponentKind::new("brought").uses::<SecretManager>();
                    c.register_async(kind, |_, _| async { Ok(ComponentFacets::empty()) });
                }),
                "uses SecretManager",
            ),
            (
                Brings(|c| c.single::<dyn ChannelCommandProvider>(None)),
                "the library brings declares ChannelCommandProvider a single",
            ),
        ];
        for (brings, what) in refusing {
            let refused = built(&brings).expect_err(what);
            let message = format!("{refused:#}");
            assert!(message.contains(what), "{message}");
        }
        /// Offers an access policy of its own, in place of the kernel's.
        struct Replaces;
        impl Library for Replaces {
            fn name(&self) -> &'static str {
                "replaces"
            }
            fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
                library.provide(Arc::new(AllowAllPolicy::new()) as Arc<dyn AccessPolicy>)
            }
        }
        let refused = TestKernel { libraries: vec![&Replaces], ..Default::default() }
            .try_build()
            .map(drop)
            .expect_err("the kernel's policy stays");
        assert!(format!("{refused:#}").contains("AccessPolicy is offered already"), "{refused:#}");
        TestKernel {
            components: Box::new(|registry| {
                let kind = ComponentKind::new("brought").uses::<dyn AccessPolicy>();
                registry.register(kind, |_, _| Ok(ComponentFacets::empty()));
            }),
            ..Default::default()
        }
        .try_build()
        .expect("the kernel's own kinds may");
    }

    /// Issue 521: a component kind registered under a name that is taken, and a single declared
    /// again, keep the kernel from being built, naming who made both - a plugin and a library, the
    /// kernel and a test alike.
    #[test]
    fn the_kernel_is_not_built_where_a_kind_or_a_single_is_registered_twice() {
        trait Twice: Send + Sync {}
        /// Registers the kind `twice` and declares `Twice` a single.
        struct Registers;
        impl Plugin for Registers {
            fn name(&self) -> &'static str {
                "registers"
            }
            fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
                kernel
                    .components()
                    .register(ComponentKind::new("twice"), |_, _| Ok(ComponentFacets::empty()));
                kernel.components().single::<dyn Twice>(None);
                Ok(())
            }
        }
        /// What a library does as it registers.
        struct Again(fn(&mut LibraryComponents<'_>));
        impl Library for Again {
            fn name(&self) -> &'static str {
                "again"
            }
            fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
                (self.0)(&mut library.components());
                Ok(())
            }
        }
        let built = |again: &Again| {
            TestKernel { plugins: vec![&Registers], libraries: vec![again], ..Default::default() }
                .try_build()
                .map(drop)
        };

        built(&Again(|_| {})).expect("everything once");
        let refusing: [(Again, &str); 2] = [
            (
                Again(|c| c.register(ComponentKind::new("twice"), |_, _| unreachable!())),
                "The component kind 'twice' is registered by the plugin registers and again by \
                 the library again",
            ),
            (
                Again(|c| c.single::<dyn Twice>(None)),
                "Twice is declared a single by the plugin registers and again by the library \
                 again",
            ),
        ];
        for (again, what) in refusing {
            let refused = built(&again).expect_err(what);
            assert_eq!(refused.to_string(), what, "{refused:#}");
        }
        let refused = TestKernel {
            components: Box::new(|registry| {
                registry.register(ComponentKind::new(HELP_KIND), |_, _| unreachable!());
            }),
            ..Default::default()
        }
        .try_build()
        .map(drop)
        .expect_err("named as the kernel's");
        assert_eq!(
            refused.to_string(),
            "The component kind 'help' is registered by the kernel and again by the test",
            "{refused:#}"
        );
    }

    /// Issue 521: a service of a type offered already, and a kind of agent registered already, are
    /// refused, naming the plugin or the library that offered or registered it again.
    #[test]
    fn a_service_or_a_kind_of_agent_registered_again_is_refused() {
        /// A service of a plugin.
        #[derive(Debug)]
        struct Offered;
        /// Offers `Offered`.
        struct Offers(&'static str);
        impl Plugin for Offers {
            fn name(&self) -> &'static str {
                self.0
            }
            fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
                kernel.provide(Arc::new(Offered))
            }
        }
        let refused =
            TestKernel { plugins: vec![&Offers("first"), &Offers("second")], ..Default::default() }
                .try_build()
                .map(drop)
                .expect_err("offered twice");
        assert_eq!(
            format!("{refused:#}"),
            "Cannot register the plugin second: The service Offered is offered already",
            "{refused:#}"
        );

        let answering = ScriptedAgents(Script::Answers("ok".to_string()));
        let refused = TestKernel { libraries: vec![&answering, &answering], ..Default::default() }
            .try_build()
            .map(drop)
            .expect_err("registered twice");
        assert_eq!(
            format!("{refused:#}"),
            "Cannot register the library scripted_agents: The kind of agent Internal is registered \
             already",
            "{refused:#}"
        );
    }

    /// Issue 516: a component kind that needs an extension no plugin binds keeps the kernel from
    /// being built - bound as the other face, it is not bound for it -, whether or not the binding
    /// offers it anywhere.
    #[test]
    fn the_kernel_is_not_built_where_a_kind_needs_an_extension_no_plugin_binds() {
        trait Ledger: Send + Sync {}
        /// Binds a `Ledger` that is offered nowhere, as the face it holds, if any.
        struct BindsLedger(Option<Face>);
        impl Plugin for BindsLedger {
            fn name(&self) -> &'static str {
                "ledger"
            }
            fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
                match self.0 {
                    Some(Face::Reader) => {
                        kernel.extend_readers::<dyn Ledger, _, _>(|_| async { None })
                    }
                    Some(Face::Writer) => {
                        kernel.extend_writers::<dyn Ledger, _, _>(|_| async { None })
                    }
                    None => Ok(()),
                }
            }
        }
        let built = |binds: &BindsLedger| {
            TestKernel {
                components: Box::new(|registry| {
                    registry.register(
                        ComponentKind::new("ledger_reader").reads::<dyn Ledger>(),
                        |_, _| Ok(ComponentFacets::empty()),
                    );
                }),
                plugins: vec![binds],
                ..Default::default()
            }
            .try_build()
            .map(|_| ())
        };

        let refused = built(&BindsLedger(None)).expect_err("bound by none");
        let message = format!("{refused:#}");
        assert!(
            message.contains("'ledger_reader'") && message.contains("reader Ledger"),
            "{message}"
        );
        assert!(built(&BindsLedger(Some(Face::Writer))).is_err(), "a writer is no reader");
        built(&BindsLedger(Some(Face::Reader))).expect("bound, though offered nowhere");
    }

    /// Issue 57: the variants of the texts the configuration names are read as the kernel is built,
    /// after every plugin added its texts, and one that does not fit keeps it from being built.
    #[tokio::test]
    async fn the_variants_of_the_texts_are_read_when_the_kernel_is_built() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("default")).unwrap();
        fs::write(
            dir.path().join("default").join("kernel.yaml"),
            "given_up_cause:\n  failed: it broke\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("gemma4")).unwrap();
        fs::write(
            dir.path().join("gemma4").join("kernel.yaml"),
            "given_up_cause:\n  left: you went\n",
        )
        .unwrap();
        let yaml = |variants: &str| format!("prompts:\n{variants}");
        let config = yaml(&format!(
            "  gemma4: \"{0}/gemma4\"\n  default: \"{0}/default\"\n",
            dir.path().display()
        ));
        let kernel = KernelBuilder::from_config(&AppConfig::from_yaml(&config).unwrap(), &[])
            .await
            .unwrap()
            .build()
            .unwrap();
        let prompts = kernel.prompts();
        assert_eq!(
            prompts.render("kernel.given_up_cause.failed", ()).unwrap(),
            "it broke",
            "the default overridden"
        );
        assert_eq!(
            prompts
                .render_in(
                    Some(PromptVariant::new("gemma4").unwrap()),
                    "kernel.given_up_cause.left",
                    ()
                )
                .unwrap(),
            "you went"
        );
        assert_eq!(
            prompts
                .render_in(
                    Some(PromptVariant::new("gemma4").unwrap()),
                    "kernel.given_up_cause.failed",
                    ()
                )
                .unwrap(),
            "it broke",
            "a variant falls back to the overridden default"
        );

        fs::write(dir.path().join("gemma4").join("kernel.yaml"), "given_up_cause:\n  gone: away\n")
            .unwrap();
        let refused = KernelBuilder::from_config(&AppConfig::from_yaml(&config).unwrap(), &[])
            .await
            .unwrap()
            .build()
            .map(|_| ())
            .expect_err("no such text");
        assert!(format!("{refused:#}").contains("'kernel.given_up_cause.gone'"), "{refused:#}");
    }

    /// Issue 518: what a plugin keeps behind an agent goes when the agent is purged, after the
    /// kernel's own state of it; where it cannot be removed, the agent stays deleted, and the next
    /// purge tries again.
    #[tokio::test]
    async fn what_a_plugin_keeps_behind_an_agent_goes_when_the_agent_is_purged() {
        use crate::agent::AgentMember;
        use caelumora_domain::actor::ActorId;
        use caelumora_domain::agent::testing::make_personality;
        use caelumora_domain::agent::{Agent, AgentId, AgentState};
        use caelumora_domain::confidentiality::Classification;
        use caelumora_domain::memory::{MemoryChange, MemoryId};
        use caelumora_domain::message::MessageId;
        use caelumora_domain::store::StoreError;
        use caelumora_domain::user::UserId;
        use caelumora_security::SecurityContext;
        use caelumora_store::testing::stored_agent;
        use caelumora_store::StateStore;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Mutex;

        /// Records the agents whose purge reached it, and whether their state was gone by then;
        /// fails while `failing` is set.
        struct Kept {
            state_store: Arc<dyn StateStore>,
            removed: Mutex<Vec<(AgentId, bool)>>,
            failing: AtomicBool,
        }
        #[async_trait::async_trait]
        impl AgentMember for Kept {
            async fn remove_behind(&self, agent: &Agent) -> StoreResult<()> {
                if self.failing.load(Ordering::SeqCst) {
                    return Err(StoreError::Backend("not now".to_string()));
                }
                let state = self.state_store.get_agent_state(agent.id).await?;
                self.removed.lock().unwrap().push((agent.id, state.memory.current().is_empty()));
                Ok(())
            }
        }
        /// Keeps `Kept` behind every agent.
        struct Keeps(Arc<Kept>);
        impl Plugin for Keeps {
            fn name(&self) -> &'static str {
                "keeps"
            }
            fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
                kernel.agent_member(Arc::clone(&self.0));
                Ok(())
            }
        }

        let store = Persistence::in_memory();
        let kept = Arc::new(Kept {
            state_store: store.state_store(),
            removed: Mutex::new(Vec::new()),
            failing: AtomicBool::new(true),
        });
        let plugin = Keeps(Arc::clone(&kept));
        let kernel = TestKernel { store, plugins: vec![&plugin], ..Default::default() }.build();
        let owner = ActorId::User(UserId::new());
        let personality = make_personality("helper");
        let agent = stored_agent(kernel.entity_stores(), "helper", personality, owner).await;
        let mut state = AgentState::default();
        let classification = Classification::CHANNEL_DEFAULT;
        let remembered = MemoryChange::Add { id: MemoryId(1), text: "kept".into(), classification };
        state.memory.push(remembered, Default::default(), MessageId::default());
        kernel.state_store().put_agent_state(agent.id, &mut state).await.unwrap();
        let agents = kernel.agent_manager();
        let system = SecurityContext::system();
        assert!(agents.delete(&system, agent.id).await.unwrap());

        agents.purge(&system, agent.id).await.expect_err("what it keeps cannot be removed");
        assert!(kept.removed.lock().unwrap().is_empty(), "nothing removed");
        assert!(
            kernel.entity_stores().agents.get_deleted(&system, agent.id).await.is_ok(),
            "the agent stays deleted"
        );

        kept.failing.store(false, Ordering::SeqCst);
        assert!(agents.purge(&system, agent.id).await.expect("purged the second time"));
        assert_eq!(*kept.removed.lock().unwrap(), [(agent.id, true)], "after the agent's state");
    }
}
