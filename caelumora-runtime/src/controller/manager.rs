//! `ControllerManager`: where the runtime gets its controllers - made for
//! the documents of a running session, or shared in this instance for
//! agents, personalities, users, workspaces and projects (issues 121, 406,
//! 467).

use super::{
    AgentController, ChannelController, PersonalityController, ProjectController,
    SessionController, UserController, WorkspaceController,
};
use crate::backend::NamedBackends;
use crate::event::EventBus;
use crate::terminal::TerminalProvider;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, PersonalityId};
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::confidentiality::ConfidentialityPolicy;
use caelumora_model::event::{EntityChange, GlobalEvent};
use caelumora_model::project::{Project, ProjectId};
use caelumora_model::security::{AccessPolicy, Secured, SecurityAttributes, SecurityContext};
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::{EntityStores, StateStore, StoreResult, StoreResultExt};
use caelumora_model::user::{User, UserId, UserProfile};
use caelumora_model::workspace::{Workspace, WorkspaceId};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Mutex;
use tracing::warn;

/// The controllers of one kind that are alive in this instance, by id.
/// Weak: whoever uses a controller - the actors - keeps it alive, and one
/// nobody uses is loaded again the next time.
type Registry<K, C> = Arc<Mutex<HashMap<K, Weak<C>>>>;

/// Where the runtime gets its controllers, in two ways that differ on
/// purpose:
///
/// - `new_…_controller(document)` makes a new controller for a document
///   that belongs to a running session - its `Session` and `Channel`s. The
///   caller has loaded the document, with its own permissions, and the
///   session that runs it owns the controller; that there is only one per
///   document follows from a session running only once (`SessionManager`).
/// - `…_controller(id)` gives the controller of an agent, a personality, a
///   user, a workspace or a project that this instance shares among all
///   that use it (issues 121, 344, 406, 467): the one that lives,
///   or loaded from the store if none does. Hence an id rather than a
///   document - whether to load is the manager's to decide - and `async`
///   and fallible.
#[derive(Debug)]
pub struct ControllerManager {
    stores: EntityStores,
    event_bus: Arc<EventBus>,
    /// Who may do what, and what may flow where: a workspace's controller
    /// asks who may change its files, and of a new classification (issue
    /// 304).
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// The storages of workspaces, by name, shared with the
    /// `WorkspaceManager` and the `CommandReaper`: a workspace's controller
    /// holds its own.
    storages: Arc<NamedBackends<dyn TerminalProvider>>,
    /// The states and their histories: a user's controller keeps what the
    /// user read of each channel (issue 404).
    states: Arc<dyn StateStore>,
    histories: Arc<dyn HistoryStore>,
    agents: Registry<AgentId, AgentController>,
    personalities: Registry<PersonalityId, PersonalityController>,
    users: Registry<UserId, UserController>,
    workspaces: Registry<WorkspaceId, WorkspaceController>,
    projects: Registry<ProjectId, ProjectController>,
    /// The task that takes announced changes into the live agent, user,
    /// workspace and project controllers, started with the first of them.
    follower: OnceLock<()>,
}

impl ControllerManager {
    pub fn new(
        stores: EntityStores,
        event_bus: Arc<EventBus>,
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        storages: Arc<NamedBackends<dyn TerminalProvider>>,
        states: Arc<dyn StateStore>,
        histories: Arc<dyn HistoryStore>,
    ) -> Self {
        Self {
            stores,
            event_bus,
            access_policy,
            confidentiality,
            storages,
            states,
            histories,
            agents: Default::default(),
            personalities: Default::default(),
            users: Default::default(),
            workspaces: Default::default(),
            projects: Default::default(),
            follower: OnceLock::new(),
        }
    }

    pub fn new_session_controller(&self, session: Session) -> Arc<SessionController> {
        Arc::new(SessionController::new(
            session,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
        ))
    }

    pub fn new_channel_controller(&self, channel: Channel) -> Arc<ChannelController> {
        Arc::new(ChannelController::new(
            channel,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
        ))
    }

    /// The controller of agent `id` in this instance: the one its actors
    /// hold, or loaded from the store if none does - once, however many ask
    /// at the same time. Read as the runtime: whoever asks for it builds an
    /// actor of an agent whose taking part was permitted when it joined.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the agent no longer exists, or whatever
    /// else the store reports.
    pub async fn agent_controller(&self, id: AgentId) -> StoreResult<Arc<AgentController>> {
        self.start_follower();
        let mut agents = self.agents.lock().await;
        if let Some(controller) = agents.get(&id).and_then(Weak::upgrade) {
            return Ok(controller);
        }
        let agent = self.stores.agents.get(&SecurityContext::system(), id).await?;
        // Its owner's roles are what it acts with (issue 304): an owner who
        // is gone leaves it none.
        let owner = match agent.owner() {
            Some(owner) => self.user_controller(owner).await.optional()?,
            None => None,
        };
        let controller = Arc::new(AgentController::new(
            agent,
            owner,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
        ));
        remember(&mut agents, id, &controller);
        Ok(controller)
    }

    /// The controller of personality `id` in this instance, like
    /// `agent_controller`: what the actors of its agents are built from and
    /// answer by (issue 406). Personalities are changed in the store, and no
    /// change of one is announced: the personality is read as stored now
    /// every time its controller is asked for, and taken over into the one
    /// that lives, for every running agent of it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the personality no longer exists - the
    /// controller that lives, if one does, then keeps it as it was for the
    /// agents that run -, or whatever else the store reports.
    pub async fn personality_controller(
        &self,
        id: PersonalityId,
    ) -> StoreResult<Arc<PersonalityController>> {
        let mut personalities = self.personalities.lock().await;
        let personality = self.stores.personalities.get(&SecurityContext::system(), id).await?;
        if let Some(controller) = personalities.get(&id).and_then(Weak::upgrade) {
            controller.take_over(personality);
            return Ok(controller);
        }
        let controller = Arc::new(PersonalityController::new(personality));
        remember(&mut personalities, id, &controller);
        Ok(controller)
    }

    /// The controller of user `id` in this instance, like
    /// `agent_controller`.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the user no longer exists, or whatever
    /// else the store reports.
    pub async fn user_controller(&self, id: UserId) -> StoreResult<Arc<UserController>> {
        self.start_follower();
        let mut users = self.users.lock().await;
        if let Some(controller) = users.get(&id).and_then(Weak::upgrade) {
            return Ok(controller);
        }
        let (user, profile) = UserController::load_stored(
            self.stores.users.as_ref(),
            self.stores.user_profiles.as_ref(),
            id,
        )
        .await?;
        let controller = Arc::new(UserController::new(
            user,
            profile,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
            Arc::clone(&self.states),
            Arc::clone(&self.histories),
        ));
        remember(&mut users, id, &controller);
        Ok(controller)
    }

    /// The controller of workspace `id` in this instance, like
    /// `agent_controller`: with its storage, if that is still configured.
    /// Read as the runtime: whoever changes or uses the workspace through
    /// it is asked as they do.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the workspace no longer exists, or
    /// whatever else the store reports.
    pub async fn workspace_controller(
        &self,
        id: WorkspaceId,
    ) -> StoreResult<Arc<WorkspaceController>> {
        self.start_follower();
        let mut workspaces = self.workspaces.lock().await;
        if let Some(controller) = workspaces.get(&id).and_then(Weak::upgrade) {
            return Ok(controller);
        }
        let workspace = self.stores.workspaces.get(&SecurityContext::system(), id).await?;
        let provider = self.storages.get(&workspace.storage).cloned();
        let controller = Arc::new(WorkspaceController::new(
            workspace,
            provider,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
            Arc::clone(&self.access_policy),
            Arc::clone(&self.confidentiality),
        ));
        remember(&mut workspaces, id, &controller);
        Ok(controller)
    }

    /// The controller of project `id` in this instance, like
    /// `workspace_controller`. Read as the runtime: whoever changes the
    /// project through it is asked as they do.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the project no longer exists, or whatever
    /// else the store reports.
    pub async fn project_controller(&self, id: ProjectId) -> StoreResult<Arc<ProjectController>> {
        self.start_follower();
        let mut projects = self.projects.lock().await;
        if let Some(controller) = projects.get(&id).and_then(Weak::upgrade) {
            return Ok(controller);
        }
        let project = self.stores.projects.get(&SecurityContext::system(), id).await?;
        let controller = Arc::new(ProjectController::new(
            project,
            Arc::<EventBus>::clone(&self.event_bus),
            &self.stores,
            Arc::clone(&self.access_policy),
        ));
        remember(&mut projects, id, &controller);
        Ok(controller)
    }

    /// The context in which what `owner` owns is built, in `channel` of
    /// `session`: a channel's components, a session's commands (issue
    /// 321). A user's as their controller gives it, with their roles as they
    /// are now; a user who no longer exists, or an owner that is no user,
    /// with none - as an agent whose owner is gone (`agent_controller`).
    ///
    /// # Errors
    /// Whatever the store reports, other than that the user is gone.
    pub async fn owner_context(
        &self,
        owner: ActorId,
        session: SessionId,
        channel: ChannelId,
    ) -> StoreResult<SecurityContext> {
        let user = match owner {
            ActorId::User(id) => self.user_controller(id).await.optional()?,
            _ => None,
        };
        Ok(match user {
            Some(user) => user.security_context(session, channel),
            None => SecurityContext::new(owner, session, channel),
        })
    }

    /// The name `actor` is shown with: its fixed one (`ActorId::fixed_name`),
    /// or its user's or agent's current one, from the controller this
    /// instance shares - an actor that runs holds it already.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the user or agent no longer exists, or
    /// whatever else the store reports.
    pub async fn actor_name(&self, actor: ActorId) -> StoreResult<String> {
        match actor {
            ActorId::User(id) => Ok(self.user_controller(id).await?.username()),
            ActorId::Agent(id) => Ok(self.agent_controller(id).await?.name()),
            other => Ok(other.fixed_name().unwrap_or_default().to_string()),
        }
    }

    /// The attributes of agent `id` as stored, from the controller this
    /// instance shares - what is asked of an agent named in an event, never
    /// as the event says, since the sender makes that itself.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the agent no longer exists, or whatever
    /// else the store reports.
    pub async fn agent_attributes(&self, id: AgentId) -> StoreResult<SecurityAttributes> {
        Ok(self.agent_controller(id).await?.load().security_attributes.clone())
    }

    /// Starts the task that takes the changes announced about agents,
    /// users, workspaces and projects into their live controllers - once per instance, not once per
    /// session that uses them, and the one place where changes made in
    /// other instances will arrive. Started before the first controller is
    /// loaded, so that nothing announced after loading is missed; an event
    /// that arrives while a controller is being loaded waits for it, since
    /// the registry is locked meanwhile. Events it fell behind on are made up
    /// for by reading every live controller anew (issue 502).
    fn start_follower(&self) {
        self.follower.get_or_init(|| {
            let mut events = self.event_bus.subscribe_all().into_receiver();
            let follower = Follower {
                agents: Arc::clone(&self.agents),
                users: Arc::clone(&self.users),
                workspaces: Arc::clone(&self.workspaces),
                projects: Arc::clone(&self.projects),
            };
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => follower.take(&event).await,
                        Err(RecvError::Lagged(lost)) => {
                            warn!("The controllers lost {lost} events and are read again");
                            follower.catch_up().await;
                        }
                        Err(RecvError::Closed) => return,
                    }
                }
            });
        });
    }
}

/// What follows the event bus for the live controllers this instance
/// shares: the registries of the kinds whose changes are announced.
struct Follower {
    agents: Registry<AgentId, AgentController>,
    users: Registry<UserId, UserController>,
    workspaces: Registry<WorkspaceId, WorkspaceController>,
    projects: Registry<ProjectId, ProjectController>,
}

impl Follower {
    /// Takes `event` into the live controller it is about, if one lives.
    async fn take(&self, event: &GlobalEvent) {
        if let Some((id, EntityChange::Updated(_))) = event.about::<Agent>() {
            if let Some(controller) = live(&self.agents, &id).await {
                controller.process_global_event(event);
            }
        } else if let Some((id, EntityChange::Updated(_))) =
            event.about::<User>().or_else(|| event.about::<UserProfile>())
        {
            if let Some(controller) = live(&self.users, &id).await {
                controller.process_global_event(event);
            }
        } else if let Some((id, EntityChange::Updated(_))) = event.about::<Workspace>() {
            // Read again as stored, without holding the registry.
            if let Some(controller) = live(&self.workspaces, &id).await {
                if let Err(e) = controller.process_global_event(event).await {
                    warn!(workspace = %id, "A change of the workspace made elsewhere could not be taken over: {e}");
                }
            }
        } else if let Some((id, EntityChange::Updated(_))) = event.about::<Project>() {
            // Read again as stored, as a workspace is.
            if let Some(controller) = live(&self.projects, &id).await {
                if let Err(e) = controller.process_global_event(event).await {
                    warn!(project = %id, "A change of the project made elsewhere could not be taken over: {e}");
                }
            }
        }
    }

    /// Reads every live controller anew as stored, for the changes whose
    /// announcements were lost. One that cannot be read is logged and stays
    /// as it was.
    async fn catch_up(&self) {
        for controller in all_live(&self.agents).await {
            if let Err(e) = controller.reload().await {
                warn!(agent = %controller.id(), "The agent could not be read again: {e}");
            }
        }
        for controller in all_live(&self.users).await {
            if let Err(e) = controller.reload().await {
                warn!(user = %controller.id(), "The user could not be read again: {e}");
            }
        }
        for controller in all_live(&self.workspaces).await {
            if let Err(e) = controller.reload().await {
                warn!(workspace = %controller.id(), "The workspace could not be read again: {e}");
            }
        }
        for controller in all_live(&self.projects).await {
            if let Err(e) = controller.reload().await {
                warn!(project = %controller.id(), "The project could not be read again: {e}");
            }
        }
    }
}

/// The controller under `id` in `registry`, if it lives; the registry is not
/// held once it is found.
async fn live<K: Eq + Hash, C>(registry: &Registry<K, C>, id: &K) -> Option<Arc<C>> {
    registry.lock().await.get(id).and_then(Weak::upgrade)
}

/// Every controller in `registry` that lives; the registry is not held
/// while they are used.
async fn all_live<K, C>(registry: &Registry<K, C>) -> Vec<Arc<C>> {
    registry.lock().await.values().filter_map(Weak::upgrade).collect()
}

/// Registers `controller` under `id`, dropping the entries of controllers
/// nobody holds any more.
fn remember<K: Eq + Hash, C>(registry: &mut HashMap<K, Weak<C>>, id: K, controller: &Arc<C>) {
    registry.retain(|_, c| c.strong_count() > 0);
    registry.insert(id, Arc::downgrade(controller));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::StorageGone;
    use crate::workspace::WorkspaceError;
    use caelumora_model::agent::testing::{make_personality, stored_agent};
    use caelumora_model::agent::{Agent, AgentUpdateEvent, Personality, PersonalityId};
    use caelumora_model::approval::ApproverId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::project::ProjectUpdateEvent;
    use caelumora_model::removal::Removal;
    use caelumora_model::role::RoleId;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_model::security::{AclEntry, Grant, Subject};
    use caelumora_model::store::Persistence;
    use caelumora_model::store::StoreError;
    use caelumora_model::user::testing::make_user;
    use caelumora_model::user::{User, UserUpdateEvent};
    use caelumora_model::value::language::Language;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::workspace::WorkspaceUpdateEvent;
    use std::time::Duration;
    use tokio::time;

    fn manager() -> (Persistence, Arc<EventBus>, ControllerManager) {
        let store = Persistence::in_memory();
        let bus = Arc::new(EventBus::new(
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        ));
        let manager = ControllerManager::new(
            store.entity_stores().unwrap(),
            Arc::clone(&bus),
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
            Default::default(),
            store.state_store(),
            store.history_store(),
        );
        (store, bus, manager)
    }

    async fn agent_in(store: &Persistence) -> Agent {
        stored_agent(
            &store.entity_stores().unwrap(),
            "helper",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await
    }

    async fn user_in(store: &Persistence) -> User {
        let mut user = make_user("alice");
        store
            .entity_stores()
            .unwrap()
            .users
            .create(&SecurityContext::system(), &mut user)
            .await
            .unwrap();
        user
    }

    /// A workspace of a user of its own, in the storage named `storage`.
    async fn workspace_in(store: &Persistence, storage: &str) -> Workspace {
        let owner = UserId::new();
        let mut workspace = Workspace::new(
            ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap(),
            storage,
            Default::default(),
            owner,
        );
        store
            .entity_stores()
            .unwrap()
            .workspaces
            .create(&SecurityContext::system(), &mut workspace)
            .await
            .unwrap();
        workspace
    }

    /// A project of a user of its own.
    async fn project_in(store: &Persistence) -> Project {
        let owner = UserId::new();
        let mut project = Project::new(
            ScopedName::new("report", Scope::UserScope { user_id: owner }).unwrap(),
            owner,
        );
        store
            .entity_stores()
            .unwrap()
            .projects
            .create(&SecurityContext::system(), &mut project)
            .await
            .unwrap();
        project
    }

    #[tokio::test]
    async fn an_actor_is_named_by_its_entity_or_by_what_it_is() {
        let (store, _, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);
        let approver = ActorId::Approver(ApproverId::new());

        let mut names = Vec::new();
        for actor in [ActorId::User(user.id), ActorId::Agent(agent.id), approver, ActorId::System] {
            names.push(manager.actor_name(actor).await.unwrap());
        }

        assert_eq!(names, ["alice", "helper", "Approver", "system"]);
        assert!(
            matches!(
                manager.actor_name(ActorId::User(UserId::new())).await,
                Err(StoreError::NotFound { .. })
            ),
            "a user who does not exist has no name"
        );
    }

    /// Waits until `done` holds, as the follower works in a task of its own.
    async fn eventually(done: impl Fn() -> bool) {
        time::timeout(Duration::from_secs(2), async {
            while !done() {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("in time");
    }

    /// Issue 502: changes whose announcements the follower lost are read
    /// again as stored, into every live controller.
    #[tokio::test]
    async fn changes_whose_announcements_were_lost_are_read_again() {
        let (store, bus, manager) = manager();
        let agent = agent_in(&store).await;
        let user = user_in(&store).await;
        let workspace = workspace_in(&store, "main").await;
        let project = project_in(&store).await;
        let agents = manager.agent_controller(agent.id).await.unwrap();
        let users = manager.user_controller(user.id).await.unwrap();
        let workspaces = manager.workspace_controller(workspace.id).await.unwrap();
        let projects = manager.project_controller(project.id).await.unwrap();
        let stores = store.entity_stores().unwrap();
        let system = SecurityContext::system();

        // Changed in the store, as elsewhere, and not announced here.
        let mut renamed = agent.clone();
        renamed.name.name = Name::new("renamed").unwrap();
        stores.agents.update(&system, &mut renamed).await.unwrap();
        let mut promoted = user.clone();
        promoted.roles = vec![RoleId::new()];
        stores.users.update(&system, &mut promoted).await.unwrap();
        let mut profile = UserProfile::new(user.id, None);
        profile.preferred_language = Language::optional(Some("de")).unwrap();
        stores.user_profiles.create(&system, &mut profile).await.unwrap();
        let mut notes = workspace.clone();
        notes.name.name = Name::new("notes").unwrap();
        stores.workspaces.update(&system, &mut notes).await.unwrap();
        let mut annual = project.clone();
        annual.name.name = Name::new("annual").unwrap();
        stores.projects.update(&system, &mut annual).await.unwrap();
        // Published before the follower runs again: it falls behind.
        bus.publish_past_capacity(&GlobalEvent::created::<Session>(&SessionId::new()));

        eventually(|| {
            agents.name() == "renamed"
                && users.load().roles == promoted.roles
                && users.profile().preferred_language == profile.preferred_language
                && workspaces.load().name.name.as_str() == "notes"
                && projects.load().name.name.as_str() == "annual"
        })
        .await;
    }

    #[tokio::test]
    async fn an_agent_and_a_user_have_one_controller_in_the_instance_while_it_is_used() {
        let (store, _, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);

        let first = manager.agent_controller(agent.id).await.unwrap();
        assert!(
            Arc::ptr_eq(&first, &manager.agent_controller(agent.id).await.unwrap()),
            "every session gets the same agent controller"
        );
        assert!(
            Arc::ptr_eq(
                &manager.user_controller(user.id).await.unwrap(),
                &manager.user_controller(user.id).await.unwrap()
            ),
            "and the same user controller"
        );

        // Nobody uses it any more: the next one is loaded again, as stored now.
        drop(first);
        let mut renamed = agent.clone();
        renamed.name.name = Name::new("renamed").unwrap();
        store
            .entity_stores()
            .unwrap()
            .agents
            .update(&SecurityContext::system(), &mut renamed)
            .await
            .unwrap();
        assert_eq!(manager.agent_controller(agent.id).await.unwrap().name(), "renamed");

        let gone = manager.agent_controller(AgentId::new()).await;
        assert!(
            matches!(gone, Err(StoreError::NotFound { .. })),
            "an agent that does not exist has none, got {gone:?}"
        );
    }

    /// Issue 406: the actors of a personality's agents answer from one copy
    /// of it, which takes in what is stored whenever the controller is asked
    /// for.
    #[tokio::test]
    async fn a_personality_has_one_controller_in_the_instance_while_it_is_used() {
        let (store, _, manager) = manager();
        let agent = agent_in(&store).await;
        let system = SecurityContext::system();

        let first = manager.personality_controller(agent.personality_id).await.unwrap();
        assert!(
            Arc::ptr_eq(
                &first,
                &manager.personality_controller(agent.personality_id).await.unwrap()
            ),
            "every agent of it gets the same controller"
        );

        let mut narrowed = Personality::clone(&first.load());
        narrowed.roles = Some(vec![RoleId::new()]);
        store.entity_stores().unwrap().personalities.update(&system, &mut narrowed).await.unwrap();
        assert_eq!(first.load().roles, None, "not taken in by itself");
        manager.personality_controller(agent.personality_id).await.unwrap();
        assert_eq!(
            first.load().roles,
            narrowed.roles,
            "taken in when asked for, for all that hold it"
        );

        store
            .entity_stores()
            .unwrap()
            .personalities
            .delete(&system, agent.personality_id, &Removal::now(system.caller_id))
            .await
            .unwrap();
        let deleted = manager.personality_controller(agent.personality_id).await;
        assert!(
            matches!(deleted, Err(StoreError::NotFound { .. })),
            "a deleted one is not handed out, got {deleted:?}"
        );
        assert_eq!(first.load().roles, narrowed.roles, "but kept for those that hold it");

        let gone = manager.personality_controller(PersonalityId::new()).await;
        assert!(
            matches!(gone, Err(StoreError::NotFound { .. })),
            "a personality that does not exist has none, got {gone:?}"
        );
    }

    #[tokio::test]
    async fn announced_changes_reach_the_live_controllers() {
        let (store, bus, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);
        let agents_controller = manager.agent_controller(agent.id).await.unwrap();
        let users_controller = manager.user_controller(user.id).await.unwrap();

        // As another instance, or whoever changes users, would announce them.
        bus.publish(
            GlobalEvent::updated::<Agent>(
                &agent.id,
                &AgentUpdateEvent::NameChanged("from elsewhere".to_string()),
            ),
            SecurityAttributes::systems(),
        );
        bus.publish(
            GlobalEvent::updated::<User>(
                &user.id,
                &UserUpdateEvent::NameChanged("alice b.".to_string()),
            ),
            SecurityAttributes::systems(),
        );

        eventually(|| {
            agents_controller.name() == "from elsewhere"
                && users_controller.username() == "alice b."
        })
        .await;
    }

    #[tokio::test]
    async fn a_change_made_here_is_not_undone_when_its_announcement_comes_back() {
        let (store, _, manager) = manager();
        let agent = agent_in(&store).await;
        let controller = manager.agent_controller(agent.id).await.unwrap();

        controller.update_name(&SecurityContext::system(), "first").await.unwrap();
        controller.update_name(&SecurityContext::system(), "second").await.unwrap();
        // Both announcements come back through the follower; each carries
        // the name it set, so the last one leaves the last name.
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(controller.name(), "second");
    }

    /// A workspace has one controller in the instance, however many use it
    /// (issue 344); a change made elsewhere reaches it as stored - an access
    /// list too, which its announcement does not carry.
    #[tokio::test]
    async fn a_workspace_has_one_controller_which_takes_over_what_was_changed_elsewhere() {
        let (store, bus, manager) = manager();
        let workspace = workspace_in(&store, "main").await;
        let controller = manager.workspace_controller(workspace.id).await.unwrap();
        assert!(
            Arc::ptr_eq(&controller, &manager.workspace_controller(workspace.id).await.unwrap()),
            "every session gets the same one"
        );

        // As another instance would change it: stored, then announced.
        let mut changed = workspace.clone();
        changed.name.name = Name::new("notes").unwrap();
        changed.security_attributes.acl = vec![AclEntry {
            subject: Subject::Actor(ActorId::User(UserId::new())),
            grant: Grant::Read,
        }];
        store
            .entity_stores()
            .unwrap()
            .workspaces
            .update(&SecurityContext::system(), &mut changed)
            .await
            .unwrap();
        bus.publish(
            GlobalEvent::updated::<Workspace>(&workspace.id, &WorkspaceUpdateEvent::AccessChanged),
            SecurityAttributes::systems(),
        );

        eventually(|| {
            controller.load().name.name.as_str() == "notes"
                && controller.load().security_attributes.acl.len() == 1
        })
        .await;
        let gone = manager.workspace_controller(WorkspaceId::new()).await;
        assert!(
            matches!(gone, Err(StoreError::NotFound { .. })),
            "a workspace that does not exist has none, got {gone:?}"
        );
    }

    /// A project has one controller in the instance, however many sessions lie in it (issue
    /// 467); a change made elsewhere reaches it as stored - an access list too.
    #[tokio::test]
    async fn a_project_has_one_controller_which_takes_over_what_was_changed_elsewhere() {
        let (store, bus, manager) = manager();
        let project = project_in(&store).await;
        let controller = manager.project_controller(project.id).await.unwrap();
        assert!(
            Arc::ptr_eq(&controller, &manager.project_controller(project.id).await.unwrap()),
            "every session gets the same one"
        );

        let mut changed = project.clone();
        changed.name.name = Name::new("annual").unwrap();
        changed.security_attributes.acl = vec![AclEntry {
            subject: Subject::Actor(ActorId::User(UserId::new())),
            grant: Grant::Read,
        }];
        store
            .entity_stores()
            .unwrap()
            .projects
            .update(&SecurityContext::system(), &mut changed)
            .await
            .unwrap();
        bus.publish(
            GlobalEvent::updated::<Project>(&project.id, &ProjectUpdateEvent::AccessChanged),
            SecurityAttributes::systems(),
        );

        eventually(|| {
            controller.load().name.name.as_str() == "annual"
                && controller.load().security_attributes.acl == changed.security_attributes.acl
        })
        .await;
        let gone = manager.project_controller(ProjectId::new()).await;
        assert!(
            matches!(gone, Err(StoreError::NotFound { .. })),
            "a project that does not exist has none, got {gone:?}"
        );
    }

    /// A workspace whose storage was removed from the configuration has its controller, but no
    /// terminal: its storage is gone, which is the server's, not that the caller named a storage
    /// unknown (issue 466).
    #[tokio::test]
    async fn a_workspace_whose_storage_is_gone_has_no_terminal() {
        let (store, _, manager) = manager();
        let workspace = workspace_in(&store, "removed").await;
        let controller = manager.workspace_controller(workspace.id).await.unwrap();

        let terminal = controller.terminal(&SecurityContext::system(), Grant::Read).await;
        assert!(
            matches!(&terminal, Err(WorkspaceError::StorageGone(StorageGone(name))) if name == "removed"),
            "{:?}",
            terminal.err()
        );
    }

    /// What is built for a user is built with their roles as they are now;
    /// for one who is gone, or an owner that is no user, with none (issue
    /// 321).
    #[tokio::test]
    async fn an_owner_builds_with_the_roles_of_now() {
        let (store, _bus, manager) = manager();
        let staff = RoleId::new();
        let mut alice = User { roles: vec![staff], ..user_in(&store).await };
        store
            .entity_stores()
            .unwrap()
            .users
            .update(&SecurityContext::system(), &mut alice)
            .await
            .unwrap();
        let (session, channel) = (SessionId::new(), ChannelId::new());

        let hers = manager.owner_context(ActorId::User(alice.id), session, channel).await.unwrap();
        assert_eq!(hers.principal_id, ActorId::User(alice.id));
        assert!(hers.has_role(staff), "her roles");
        assert_eq!((hers.session_id, hers.channel_id), (session, channel));
        let gone =
            manager.owner_context(ActorId::User(UserId::new()), session, channel).await.unwrap();
        assert!(gone.roles.is_empty(), "a user who is gone has none");
        let system = manager.owner_context(ActorId::System, session, channel).await.unwrap();
        assert_eq!(system.principal_id, ActorId::System);
    }
}
