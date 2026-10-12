//! The live documents of a running session and their one writer each
//! (issue 86): a controller holds the current `Session`, `Channel`, `Agent`
//! or `User`, stores a change first - which is where the permission is
//! checked - and only then takes it over and announces it on the event bus.
//! The documents themselves are the model's; keeping them live, writing
//! them and announcing is the runtime's.
//!
//! Every controller is built the same way - the document, the event
//! publisher, the store - holds a write lock while it changes its document,
//! and takes over changes made elsewhere through `process_global_event`.
//! The user's writes the user's profile; the user record itself belongs to
//! the system and is not changed through it.
//!
//! A session and its channels run in one instance, and so do their
//! controllers. Agents and users do not belong to a session: one of them
//! takes part in several, perhaps in several instances. The
//! `ControllerManager` keeps one controller per agent and per user of its
//! instance, shared by all their actors, and takes the changes announced
//! about them into those controllers (issue 121).

mod agent;
mod channel;
mod session;
mod user;

pub use agent::AgentController;
pub use channel::ChannelController;
pub use session::SessionController;
pub use user::UserController;

use crate::event::EventBus;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::Channel;
use caelumora_model::event::EntityChange;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::Session;
use caelumora_model::store::{EntityStores, StoreError, StoreResult, StoreResultExt};
use caelumora_model::user::{User, UserId, UserProfile};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::Mutex;

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
/// - `…_controller(id)` gives the controller of an agent or user that this
///   instance shares among all its actors (issue 121): the one that lives,
///   or loaded from the store if none does. Hence an id rather than a
///   document - whether to load is the manager's to decide - and `async`
///   and fallible.
#[derive(Debug)]
pub struct ControllerManager {
    stores: EntityStores,
    event_bus: Arc<EventBus>,
    agents: Registry<AgentId, AgentController>,
    users: Registry<UserId, UserController>,
    /// The task that takes announced changes into the live agent and user
    /// controllers, started with the first of them.
    follower: OnceLock<()>,
}

impl ControllerManager {
    pub fn new(stores: EntityStores, event_bus: Arc<EventBus>) -> Self {
        Self {
            stores,
            event_bus,
            agents: Default::default(),
            users: Default::default(),
            follower: OnceLock::new(),
        }
    }

    pub fn new_session_controller(&self, session: Session) -> Arc<SessionController> {
        Arc::new(SessionController::new(session, Arc::<EventBus>::clone(&self.event_bus), Arc::clone(&self.stores.sessions)))
    }

    pub fn new_channel_controller(&self, channel: Channel) -> Arc<ChannelController> {
        Arc::new(ChannelController::new(channel, Arc::<EventBus>::clone(&self.event_bus), &self.stores))
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
        let owner = match agent.security_attributes.owner_id {
            ActorId::User(owner) => self.user_controller(owner).await.optional()?,
            _ => None,
        };
        let controller = Arc::new(AgentController::new(agent, owner, Arc::<EventBus>::clone(&self.event_bus), &self.stores));
        remember(&mut agents, id, &controller);
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
        let user = self.stores.users.get(&SecurityContext::system(), id).await?;
        let profile = self.stores.user_profiles.get(&SecurityContext::system(), id).await.optional()?;
        let controller = Arc::new(UserController::new(user, profile, Arc::<EventBus>::clone(&self.event_bus), Arc::clone(&self.stores.user_profiles)));
        remember(&mut users, id, &controller);
        Ok(controller)
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

    /// Starts the task that takes the changes announced about agents and
    /// users into their live controllers - once per instance, not once per
    /// session that uses them, and the one place where changes made in
    /// other instances will arrive. Started before the first controller is
    /// loaded, so that nothing announced after loading is missed; an event
    /// that arrives while a controller is being loaded waits for it, since
    /// the registry is locked meanwhile.
    fn start_follower(&self) {
        self.follower.get_or_init(|| {
            let mut events = self.event_bus.subscribe_all();
            let (agents, users) = (Arc::clone(&self.agents), Arc::clone(&self.users));
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if let Some((id, EntityChange::Updated(_))) = event.about::<Agent>() {
                        if let Some(controller) = agents.lock().await.get(&id).and_then(Weak::upgrade) {
                            controller.process_global_event(&event);
                        }
                    } else if let Some((id, EntityChange::Updated(_))) = event.about::<User>().or_else(|| event.about::<UserProfile>()) {
                        if let Some(controller) = users.lock().await.get(&id).and_then(Weak::upgrade) {
                            controller.process_global_event(&event);
                        }
                    }
                }
            });
        });
    }
}

/// Nothing if a place may keep every one of `rules` (issue 13): a
/// session's or a channel's rules only ask or deny, everywhere within it.
///
/// # Errors
/// `StoreError::Validation` naming the first rule that does not, and why.
fn check_restricting(rules: &[ApprovalRule]) -> StoreResult<()> {
    for rule in rules {
        rule.check_restricting().map_err(|e| StoreError::Validation(format!("the rule for `{}`: {e}", rule.matcher.tool)))?;
    }
    Ok(())
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
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::agent::{Agent, AgentUpdateEvent, Personality, PersonalityId};
    use caelumora_model::actor::ActorId;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::StoreError;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::{User, UserUpdateEvent};
    use std::time::Duration;
    use caelumora_model::confidentiality::ConfidentialityRules;

    fn manager() -> (Arc<MemoryStore>, Arc<EventBus>, ControllerManager) {
        let store = Arc::new(MemoryStore::new());
        let bus = Arc::new(EventBus::new(Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default())));
        let manager = ControllerManager::new(store.entity_stores(), Arc::clone(&bus));
        (store, bus, manager)
    }

    async fn agent_in(store: &Arc<MemoryStore>) -> Agent {
        let personality = Personality {
            description: None,
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
            deleted: None,
        };
        let agent = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, ActorId::None);
        store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    async fn user_in(store: &Arc<MemoryStore>) -> User {
        let user = User { id: UserId::new(), username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        store.entity_stores().users.create(&SecurityContext::system(), &user).await.unwrap();
        user
    }

    #[tokio::test]
    async fn an_actor_is_named_by_its_entity_or_by_what_it_is() {
        let (store, _, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);
        let approver = ActorId::Approver(caelumora_model::approval::ApproverId::new());

        let mut names = Vec::new();
        for actor in [ActorId::User(user.id), ActorId::Agent(agent.id), approver, ActorId::System] {
            names.push(manager.actor_name(actor).await.unwrap());
        }

        assert_eq!(names, ["alice", "helper", "Approver", "system"]);
        assert!(matches!(manager.actor_name(ActorId::User(UserId::new())).await, Err(StoreError::NotFound { .. })), "a user who does not exist has no name");
    }

    /// Waits until `done` holds, as the follower works in a task of its own.
    async fn eventually(done: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("in time");
    }

    #[tokio::test]
    async fn an_agent_and_a_user_have_one_controller_in_the_instance_while_it_is_used() {
        let (store, _, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);

        let first = manager.agent_controller(agent.id).await.unwrap();
        assert!(Arc::ptr_eq(&first, &manager.agent_controller(agent.id).await.unwrap()), "every session gets the same agent controller");
        assert!(Arc::ptr_eq(&manager.user_controller(user.id).await.unwrap(), &manager.user_controller(user.id).await.unwrap()), "and the same user controller");

        // Nobody uses it any more: the next one is loaded again, as stored now.
        drop(first);
        let mut renamed = agent.clone();
        renamed.name.name = Name::new("renamed").unwrap();
        store.entity_stores().agents.update(&SecurityContext::system(), &renamed).await.unwrap();
        assert_eq!(manager.agent_controller(agent.id).await.unwrap().name(), "renamed");

        let gone = manager.agent_controller(AgentId::new()).await;
        assert!(matches!(gone, Err(StoreError::NotFound { .. })), "an agent that does not exist has none, got {gone:?}");
    }

    #[tokio::test]
    async fn announced_changes_reach_the_live_controllers() {
        let (store, bus, manager) = manager();
        let (agent, user) = (agent_in(&store).await, user_in(&store).await);
        let (agents_controller, users_controller) = (manager.agent_controller(agent.id).await.unwrap(), manager.user_controller(user.id).await.unwrap());

        // As another instance, or whoever changes users, would announce them.
        bus.publish(GlobalEvent::updated::<Agent>(&agent.id, &AgentUpdateEvent::NameChanged("from elsewhere".to_string())), Default::default());
        bus.publish(GlobalEvent::updated::<User>(&user.id, &UserUpdateEvent::NameChanged("alice b.".to_string())), Default::default());

        eventually(|| agents_controller.name() == "from elsewhere" && users_controller.username() == "alice b.").await;
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
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(controller.name(), "second");
    }
}
