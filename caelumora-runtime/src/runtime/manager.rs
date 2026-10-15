use crate::channel::Participation;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::actor::AgentKinds;
use crate::plugin::Plugin;
use crate::component::{ComponentContainer, ComponentContext, ComponentRegistry};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::recorder::{BlackholeRecorder, Recorder};
use anyhow::Result;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::ChannelId;
use crate::agent::state::AgentStates;
use caelumora_model::component::ComponentConfig;
use caelumora_model::config::AppConfig;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::AccessPolicy;
use caelumora_model::user_provider::UserManager;
use crate::backend::Backends;
use caelumora_model::session::Session;
use caelumora_model::store::{ComponentStore, EntityStores, StateStore, StoreBackend};
use caelumora_model::type_map::TypeKey;
use crate::seed::{self, SeedSection};
use std::collections::BTreeMap;
use std::path::PathBuf;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::Arc;
use crate::agent::AgentManager;
use crate::group::GroupManager;
use crate::scoped::ScopedEntities;
use crate::workspace::WorkspaceManager;
use crate::workspace::reaper::CommandReaper;
use super::{RuntimeBuilder, Services};

#[derive(Debug)]
pub struct RuntimeManager {
    pub(super) access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where, and who is cleared for what (issue 304): asked
    /// beside the `AccessPolicy`, never by it.
    pub(super) confidentiality: Arc<dyn ConfidentialityPolicy>,
    pub(super) state_store: Arc<dyn StateStore>,
    pub(super) entity_stores: EntityStores,
    /// The preconfigured components, which the seed puts in.
    pub(super) component_store: Arc<dyn ComponentStore>,
    pub(super) secret_manager: Arc<SecretManager>,
    /// The one writer of user records, asked at a login (issue 306).
    pub(super) user_manager: Arc<UserManager>,
    pub(super) component_registry: Arc<ComponentRegistry>,
    /// The kinds of agent it runs, by which a channel builds an agent's
    /// actor (issue 370).
    pub(super) agent_kinds: Arc<AgentKinds>,
    pub(super) event_bus: Arc<EventBus>,
    pub(super) controller_manager: Arc<ControllerManager>,
    /// Who takes part in a channel and whether they may (issue 356).
    pub(super) participation: Arc<Participation>,
    pub(super) commands: ComponentConfig,
    /// What the system restricts of tool calls everywhere
    /// (`AppConfig::approvals`, issue 13).
    pub(super) approval_rules: Arc<[ApprovalRule]>,
    /// One owner of every running agent's `AgentState` (issue 62).
    pub(super) agent_states: Arc<AgentStates>,
    /// The one owner of workspaces and their places (issue 253).
    pub(super) workspaces: Arc<WorkspaceManager>,
    /// What ends the commands nobody may run any longer (issues 254, 344).
    pub(super) command_reaper: Arc<CommandReaper>,
    /// The one writer of teams (issue 286).
    pub(super) groups: Arc<GroupManager>,
    /// The one owner of agents' lifecycle (issue 314).
    pub(super) agents: Arc<AgentManager>,
    /// What deletes, restores and purges what lies in a scope with what
    /// holds it (issues 178, 319).
    pub(super) scoped: Arc<ScopedEntities>,
    /// The services components may use (issue 296), each given to the
    /// kinds that declare it (`ComponentKind::uses`), and the views.
    pub(super) services: Services,
    /// The sections of the seed the plugins claimed (issue 323).
    pub(super) seed_sections: BTreeMap<String, Arc<dyn SeedSection>>,
}

impl RuntimeManager {
    /// The runtime of the core with `plugins`: as `RuntimeBuilder` builds it
    /// from the same parts, with the core's backends. Without the agents'
    /// plugin (`AgentsPlugin`, issue 370) it runs no agent; the runtime's
    /// own tests run scripted ones (`testing::ScriptedAgents`).
    ///
    /// # Errors
    /// As `RuntimeBuilder::new`, `RuntimeBuilder::plugin` and
    /// `RuntimeBuilder::build`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state_store: Arc<dyn StateStore>,
        backend: impl Into<StoreBackend>,
        secret_manager: Arc<SecretManager>,
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        config: &AppConfig,
        component_registry: ComponentRegistry,
        plugins: &[&dyn Plugin],
    ) -> Result<Self> {
        let mut builder = RuntimeBuilder::new(state_store, backend, secret_manager, access_policy, confidentiality, config, component_registry, Backends::with_plugins(plugins))?;
        for plugin in plugins {
            builder.plugin(*plugin)?;
        }
        builder.build()
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
        Ok(CommandExecutor::new(Arc::clone(self.access_policy()), providers)?)
    }

    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> { &self.access_policy }

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

    /// The kinds of agent it runs (issue 370): how an agent's actor is
    /// built, and which facets its components are asked for.
    pub fn agent_kinds(&self) -> &Arc<AgentKinds> {
        &self.agent_kinds
    }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> {
        &self.component_registry
    }

    pub fn controller_manager(&self) -> &Arc<ControllerManager> {
        &self.controller_manager
    }

    /// Who takes part in a channel and whether they may (issue 356).
    pub(crate) fn participation(&self) -> &Arc<Participation> {
        &self.participation
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
