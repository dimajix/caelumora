use caelumora_model::value::name::Name;
use caelumora_model::value::ValueError;
use arc_swap::ArcSwap;
use caelumora_model::agent::{Agent, AgentId, AgentUpdateEvent, Personality};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::role::RoleId;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use super::UserController;
use caelumora_model::store::{EntityStore, EntityStores, StoreResult};
use std::sync::Arc;

/// The one writer of an agent's document in this instance, shared by all
/// the agent's actors in all sessions (issue 121).
#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    agent: ArcSwap<Agent>,
    /// The live user who owns the agent, whose roles it acts with (issue
    /// 304); none for an agent of the system, or of an owner who is gone.
    owner: Option<Arc<UserController>>,
    event_publisher: Arc<dyn EventPublisher>,
    agents: Arc<dyn EntityStore<Agent>>,
    personalities: Arc<dyn EntityStore<Personality>>,
    /// Held while the agent is changed and stored, like the other
    /// controllers.
    write_lock: tokio::sync::Mutex<()>,
}

impl AgentController {
    /// The live `agent`, owned by the user `owner` holds, if any. Made by
    /// the `ControllerManager`, which keeps one per agent.
    pub(crate) fn new(agent: Agent, owner: Option<Arc<UserController>>, event_publisher: Arc<dyn EventPublisher>, stores: &EntityStores) -> Self {
        Self {
            id: agent.id,
            agent: ArcSwap::from_pointee(agent),
            owner,
            event_publisher,
            agents: Arc::clone(&stores.agents),
            personalities: Arc::clone(&stores.personalities),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Agent>> {
        self.agent.load()
    }

    pub fn id(&self) -> AgentId {
        self.id
    }

    pub fn name(&self) -> String {
        self.agent.load().name.to_string()
    }

    /// The context the agent acts in, in `channel` of `session`: its owner's
    /// delegate (issue 255), with its owner's roles as they are now, limited
    /// to `limiting_roles` - its personality's (`Personality::limiting_roles`,
    /// issues 179, 304).
    pub fn security_context(&self, session: SessionId, channel: ChannelId, limiting_roles: Option<&[RoleId]>) -> SecurityContext {
        let roles = self.owner.as_ref().map(|owner| owner.load().roles.clone()).unwrap_or_default();
        self.agent.load().security_context(session, channel).with_roles(roles, limiting_roles.map(<[_]>::to_vec))
    }

    /// The agent's personality - what the agent is, read through
    /// `personality_id` each time, so that a change of the personality
    /// reaches it (issue 176). Read in the runtime's name, as the controller
    /// holds the agent's document: a controller is the runtime's, and
    /// whoever acts with what it hands out in someone's name checks that
    /// themselves - running the agent, whether it may read its personality
    /// (issue 191).
    ///
    /// # Errors
    /// Whatever the store reports: also that the personality is gone.
    pub async fn personality(&self) -> StoreResult<Personality> {
        let id = self.agent.load().personality_id;
        self.personalities.get(&SecurityContext::system(), id).await
    }

    /// Renames the agent on behalf of `ctx`: stored first - which is where
    /// the permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `new_name` is no name, otherwise whatever
    /// the store reports; the agent is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: &str) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let event = AgentUpdateEvent::NameChanged(new_name.to_string());
        let agent = self.apply(&event)?;
        self.agents.update(ctx, &agent).await?;
        let attributes = agent.security_attributes.clone();
        self.agent.store(Arc::new(agent));
        self.event_publisher.publish(GlobalEvent::updated::<Agent>(&self.id, &event), attributes);
        Ok(())
    }

    /// The agent with `event` applied, not yet stored anywhere; what it
    /// says is made into the agent's values here (issue 248).
    ///
    /// # Errors
    /// `ValueError` if what it says does not fit.
    fn apply(&self, event: &AgentUpdateEvent) -> Result<Agent, ValueError> {
        let mut agent = (**self.agent.load()).clone();
        match event {
            AgentUpdateEvent::NameChanged(name) => agent.name.name = Name::new(name)?,
        }
        Ok(agent)
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced. An event carries the resulting value, so taking over
    /// this controller's own change again changes nothing.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        let update = event.about::<Agent>().filter(|(aid, _)| *aid == self.id).and_then(|(_, change)| change.update::<AgentUpdateEvent>());
        if let Some(update) = update {
            // Checked where it was made; one that does not fit here is left
            // out rather than taken over half.
            match self.apply(&update) {
                Ok(agent) => self.agent.store(Arc::new(agent)),
                Err(e) => tracing::warn!(agent = %self.id, error = %e, "A change of the agent made elsewhere does not fit it"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::agent::{Personality, PersonalityId};
    use caelumora_model::scope::Scope;
    use caelumora_model::store::memory::MemoryStore;

    #[tokio::test]
    async fn a_renamed_agent_is_stored_first() {
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
        let agent = Agent::new(ScopedName::new("assistant", Scope::GlobalScope).unwrap(), &personality, caelumora_model::actor::ActorId::None);
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.entity_stores().agents.create(&ctx, &agent).await.unwrap();
        let controller = AgentController::new(agent.clone(), None, Arc::new(()), &store.entity_stores());

        controller.update_name(&ctx, "new_name").await.unwrap();

        assert_eq!(controller.name(), "new_name");
        assert_eq!(store.entity_stores().agents.get(&ctx, agent.id).await.unwrap().name.as_str(), "new_name");
    }

    /// Issue 304: an agent acts with its owner's roles as they are now - a
    /// role given or taken reaches it with the owner's controller - limited
    /// to those its personality names.
    #[tokio::test]
    async fn an_agent_acts_with_its_owners_roles_as_they_are_now_and_its_limit() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::event::GlobalEvent;
        use caelumora_model::role::RoleId;
        use caelumora_model::user::{User, UserId, UserUpdateEvent};
        let (staff, hr) = (RoleId::new(), RoleId::new());
        let alice = User { id: UserId::new(), username: caelumora_model::value::name::Name::new("alice").unwrap(), email: String::new(), roles: vec![staff], deleted: None };
        let store = Arc::new(MemoryStore::new());
        let owner = Arc::new(UserController::new(alice.clone(), None, Arc::new(()), store.entity_stores().user_profiles));
        let personality = Personality { roles: Some(vec![staff]), ..plain() };
        let agent = Agent::new(ScopedName::new("assistant", Scope::GlobalScope).unwrap(), &personality, ActorId::User(alice.id));
        let controller = AgentController::new(agent, Some(Arc::clone(&owner)), Arc::new(()), &store.entity_stores());
        let (session, channel) = (Default::default(), Default::default());

        let context = controller.security_context(session, channel, personality.limiting_roles());
        assert_eq!((context.principal_id, context.roles.clone(), context.limiting_roles.clone()), (ActorId::User(alice.id), vec![staff], Some(vec![staff])));
        owner.process_global_event(&GlobalEvent::updated::<User>(&alice.id, &UserUpdateEvent::RolesChanged(vec![staff, hr])));
        let context = controller.security_context(session, channel, personality.limiting_roles());
        assert_eq!(context.roles, [staff, hr], "her roles as they are now");
        assert!(context.has_role(staff) && !context.has_role(hr), "but only those its personality names count");
    }

    fn plain() -> Personality {
        Personality {
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
        }
    }
}
