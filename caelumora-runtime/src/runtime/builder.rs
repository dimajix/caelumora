//! `RuntimeBuilder`: what makes the `RuntimeManager` of a process - the core's services and
//! managers first, then what each plugin registers (issue 323).

use super::{RuntimeManager, Services};
use crate::actor::ActorManager;
use crate::actor::AgentKinds;
use crate::agent::state::AgentStates;
use crate::agent::AgentManager;
use crate::attachment::AttachmentManager;
use crate::backend::{Backends, NamedBackends};
use crate::channel::Participation;
use crate::component::ComponentRegistry;
use crate::content::ContentStorage;
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::group::GroupManager;
use crate::plugin::Plugin;
use crate::scoped::{ScopeHolders, ScopeMember, ScopedEntities, ScopedKind, Unowned};
use crate::seed::SeedSection;
use crate::terminal::TerminalProvider;
use crate::workspace::reaper::CommandReaper;
use crate::workspace::WorkspaceManager;
use anyhow::{bail, Context, Result};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::prompt_catalogue::{PromptCatalogue, CORE, CORE_PROMPTS, DEFAULT_VARIANT};
use caelumora_model::secret::{SecretManager, SecretProvider};
use caelumora_model::security::{AccessPolicy, Policies};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::{
    EntityStore, EntityStores, Persistence, PersistenceBackend, StoreResult, Stored,
};
use caelumora_model::user_provider::{UserManager, UserProvider};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// What builds the `RuntimeManager` (issue 323): the core's services and
/// managers, made first - their backends by `Backends`, the core's and the
/// plugins' (issue 324) -, and what the plugins register with it
/// (`Plugin::register`) - their component kinds, the stores of their kinds,
/// the services they provide, the members of a scope, their sections of
/// the configuration and of the seed. `build` makes the runtime of it.
pub struct RuntimeBuilder {
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
    attachments: Arc<AttachmentManager>,
    groups: Arc<GroupManager>,
    agents: Arc<AgentManager>,
    scope_members: Vec<Arc<dyn ScopedKind>>,
    scope_holders: Arc<ScopeHolders>,
    services: Services,
    /// The texts given to models, to which the plugins add theirs (issue
    /// 57); a service once the runtime is built.
    prompts: PromptCatalogue,
    /// The variants of the texts the configuration names, read once every
    /// plugin added its texts.
    prompt_variants: BTreeMap<String, PathBuf>,
    /// The sections of the configuration that no plugin claimed yet.
    sections: BTreeMap<String, Value>,
    /// The names of the sections of the configuration that were claimed.
    claimed: BTreeSet<String>,
    seed_sections: BTreeMap<String, Arc<dyn SeedSection>>,
}

impl fmt::Debug for RuntimeBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeBuilder")
            .field("services", &self.services.names())
            .finish_non_exhaustive()
    }
}

impl RuntimeBuilder {
    /// The builder of the runtime everything in a process shares, with the
    /// core's services and managers made. `policies` are those the whole
    /// process was configured with: the access policy decides tool calls and
    /// what may be done to a channel, the confidentiality policy what may flow
    /// where (issue 304). `persistence` keeps the entities of every kind, the
    /// core's and the plugins', and the states; the builder opens its stores
    /// by `policies`, so that they check by nothing else (issue 449).
    /// Of `config`, the runtime takes what it runs by itself, so that a
    /// binary cannot drop a configured part for a default: the global user
    /// commands (`commands`), the components that replace built-in defaults
    /// of facets of which an agent has one (`component_defaults`, issue 99),
    /// what the system restricts of tool calls (`approvals`, issue 13), the
    /// storages workspaces are made in (`workspaces`, issue 253), the
    /// storages of content (`content`, issue 282), what a message may carry
    /// (`attachments`, issue 282), the user providers
    /// (`users`) and the sections of the plugins. The component kinds agents and sessions are built
    /// from are the built-in ones (`ComponentRegistry::built_in`), resolving the bundles in the
    /// builder's stores; a plugin adds its own, and a test stand-ins, such as a model of its own
    /// (issue 130), through `components`. `backends` makes the storages and the user providers
    /// `config` names (issue 324): `Backends::with_plugins` of the plugins registered afterwards.
    ///
    /// # Errors
    /// If a store of the core's kinds cannot be opened, or a storage of
    /// workspaces or of content or a user provider cannot be made.
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
            &config.workspaces,
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
        let agents = Arc::new(AgentManager::new(
            &entity_stores,
            Arc::clone(&scope_holders),
            Arc::clone(&state_store),
            Arc::clone(&event_bus),
            Arc::clone(&access_policy),
        ));
        let scope_members: Vec<Arc<dyn ScopedKind>> = vec![
            Arc::new(Unowned(Arc::clone(&entity_stores.personalities))),
            Arc::new(Unowned(Arc::clone(&entity_stores.session_templates))),
            Arc::new(Unowned(Arc::clone(&entity_stores.component_bundles))),
            Arc::<AgentManager>::clone(&agents),
            Arc::<WorkspaceManager>::clone(&workspaces),
        ];
        let services = Services::empty()
            .with(Arc::clone(&access_policy))
            .with(Arc::clone(&confidentiality))
            .with(Arc::clone(&secret_manager))
            .with(Arc::clone(&workspaces))
            .with(Arc::clone(&attachments));
        // The core's texts, before any plugin adds its own.
        let mut prompts = PromptCatalogue::new();
        prompts.add(CORE, CORE_PROMPTS)?;
        let component_registry =
            ComponentRegistry::built_in(Arc::clone(&entity_stores.component_bundles));
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
            attachments,
            groups,
            agents,
            scope_members,
            scope_holders,
            services,
            prompts,
            prompt_variants: config.prompts.clone(),
            sections: config.plugins.clone(),
            claimed: BTreeSet::new(),
            seed_sections: BTreeMap::new(),
        })
    }

    /// The builder of the runtime as `config` describes it, with `plugins`
    /// registered: the policy, learning who belongs to a group from the
    /// store it guards (issue 23), the policy of what may flow where (issue
    /// 304), the secrets, the stores, the built-in component kinds and the
    /// backends of the core and of the plugins (issue 324) - first the
    /// plugins' backends, then what is made of them, then the rest of what
    /// the plugins bring. The secrets are made before the stores, whose
    /// backends may need one to connect, and the backends of the stores are
    /// opened last (`Persistence::open`, issue 447), once the plugins have
    /// opened the stores of their kinds.
    ///
    /// # Errors
    /// If a secret provider or a store cannot be made, a store cannot be
    /// opened, or as `new` and `plugin`.
    pub async fn from_config(config: &AppConfig, plugins: &[&dyn Plugin]) -> Result<Self> {
        let backends = Backends::with_plugins(plugins);
        // The policy is made before the store it guards, and bound to the
        // runtime's once it exists.
        let memberships = Arc::new(StoreMemberships::new());
        let access_policy =
            config.create_access_policy(Arc::<StoreMemberships>::clone(&memberships));
        let confidentiality = config.create_confidentiality_policy();
        let secret_services =
            Services::empty().with(Arc::clone(&access_policy)).with(Arc::clone(&confidentiality));
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
        let policies = Policies::new(access_policy, confidentiality);
        let mut builder =
            Self::new(persistence.clone(), secret_manager, policies, config, backends)?;
        memberships.bind(&builder.entity_stores().groups);
        for plugin in plugins {
            builder.plugin(*plugin)?;
        }
        persistence.open().await.context("Cannot open the stores")?;
        Ok(builder)
    }

    /// Registers what `plugin` brings.
    ///
    /// # Errors
    /// Whatever its registration fails with, naming the plugin.
    pub fn plugin(&mut self, plugin: &dyn Plugin) -> Result<()> {
        plugin
            .register(self)
            .with_context(|| format!("Cannot register the plugin {}", plugin.name()))
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

    /// The stores of the core's kinds.
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
    /// The texts given to models (issue 57), to add a plugin's with - under
    /// its namespace, from the file it compiles in. Offered to the
    /// component kinds that use it once the runtime is built.
    pub fn prompts(&mut self) -> &mut PromptCatalogue {
        &mut self.prompts
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

    /// Makes `member` go with what holds the scope it lies in: deleted,
    /// restored and purged with it, after the core's kinds.
    pub fn scope_member<M: ScopeMember + 'static>(&mut self, member: Arc<M>) {
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
            Some(section) => serde_json::from_value(section)
                .with_context(|| format!("Cannot read the section '{name}' of the configuration")),
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
        // The default variant first: the others fall back to it.
        let (defaults, variants): (Vec<_>, Vec<_>) = self
            .prompt_variants
            .iter()
            .partition(|(variant, _)| variant.as_str() == DEFAULT_VARIANT);
        for (variant, dir) in defaults.into_iter().chain(variants) {
            self.prompts.add_variant(variant, dir)?;
        }
        let prompts = Arc::new(self.prompts);
        self.services.insert(Arc::clone(&prompts));
        let scoped = Arc::new(ScopedEntities::new(self.scope_members));
        let participation = Arc::new(Participation::new(
            Arc::clone(&self.entity_stores.agents),
            Arc::clone(&self.controller_manager),
            &self.policies,
        ));
        Ok(RuntimeManager {
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
            agents: self.agents,
            actors: Arc::new(ActorManager::new()),
            scoped,
            services: self.services,
            prompts,
            seed_sections: self.seed_sections,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::prompt_catalogue::PromptVariant;
    use caelumora_model::user_provider::Credential;
    use std::fs;

    #[tokio::test]
    async fn the_backends_the_configuration_names_are_made_when_the_runtime_is_built() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!("secrets:\n  providers: []\nworkspaces:\n  storage:\n    main:\n      type: local\n      root: \"{0}\"\ncontent:\n  storage:\n    kept:\n      type: local\n      root: \"{0}\"\nusers:\n  providers:\n    - type: accept_any\n", dir.path().display());
        let builder =
            RuntimeBuilder::from_config(&AppConfig::from_yaml(&yaml).unwrap(), &[]).await.unwrap();
        assert!(
            builder.content_storages().get("kept").is_some(),
            "for their owners to be made with"
        );
        let runtime = builder.build().unwrap();
        assert_eq!(runtime.workspace_manager().storages().collect::<Vec<_>>(), [("main", false)]);
        let alice = runtime
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
            let refused = RuntimeBuilder::from_config(&AppConfig::from_yaml(yaml).unwrap(), &[]).await.map(|_| ()).expect_err(yaml);
            let message = format!("{refused:#}");
            assert!(names.iter().all(|name| message.contains(name)), "{yaml:?} gave: {message}");
        }
    }

    /// Issue 57: the variants of the texts the configuration names are read
    /// as the runtime is built, after every plugin added its texts, and one
    /// that does not fit keeps it from being built.
    #[tokio::test]
    async fn the_variants_of_the_texts_are_read_when_the_runtime_is_built() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("default")).unwrap();
        fs::write(
            dir.path().join("default").join("core.yaml"),
            "given_up_cause:\n  failed: it broke\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("gemma4")).unwrap();
        fs::write(
            dir.path().join("gemma4").join("core.yaml"),
            "given_up_cause:\n  left: you went\n",
        )
        .unwrap();
        let yaml = |variants: &str| format!("prompts:\n{variants}");
        let config = yaml(&format!(
            "  gemma4: \"{0}/gemma4\"\n  default: \"{0}/default\"\n",
            dir.path().display()
        ));
        let runtime = RuntimeBuilder::from_config(&AppConfig::from_yaml(&config).unwrap(), &[])
            .await
            .unwrap()
            .build()
            .unwrap();
        let prompts = runtime.prompts();
        assert_eq!(
            prompts.render("core.given_up_cause.failed", ()).unwrap(),
            "it broke",
            "the default overridden"
        );
        assert_eq!(
            prompts
                .render_in(
                    Some(PromptVariant::new("gemma4").unwrap()),
                    "core.given_up_cause.left",
                    ()
                )
                .unwrap(),
            "you went"
        );
        assert_eq!(
            prompts
                .render_in(
                    Some(PromptVariant::new("gemma4").unwrap()),
                    "core.given_up_cause.failed",
                    ()
                )
                .unwrap(),
            "it broke",
            "a variant falls back to the overridden default"
        );

        fs::write(dir.path().join("gemma4").join("core.yaml"), "given_up_cause:\n  gone: away\n")
            .unwrap();
        let refused = RuntimeBuilder::from_config(&AppConfig::from_yaml(&config).unwrap(), &[])
            .await
            .unwrap()
            .build()
            .map(|_| ())
            .expect_err("no such text");
        assert!(format!("{refused:#}").contains("'core.given_up_cause.gone'"), "{refused:#}");
    }
}
