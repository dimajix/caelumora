use super::document::LiveDocument;
use super::UserController;
use arc_swap::Guard;
use caelumora_model::agent::{Agent, AgentId, AgentUpdateEvent};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::role::RoleId;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::{EntityStores, StoreResult};
use std::sync::Arc;

/// The one writer of an agent's document in this instance, shared by all
/// the agent's actors in all sessions (issue 121).
#[derive(Debug)]
pub struct AgentController {
    agent: LiveDocument<Agent>,
    /// The live user who owns the agent, whose roles it acts with (issue
    /// 304); none for an owner that is no user, or is gone.
    owner: Option<Arc<UserController>>,
}

impl AgentController {
    /// The live `agent`, owned by the user `owner` holds, if any. Made by
    /// the `ControllerManager`, which keeps one per agent.
    pub fn new(
        agent: Agent,
        owner: Option<Arc<UserController>>,
        event_publisher: Arc<dyn EventPublisher>,
        stores: &EntityStores,
    ) -> Self {
        Self {
            agent: LiveDocument::new(agent.id, agent, Arc::clone(&stores.agents), event_publisher),
            owner,
        }
    }

    pub fn load(&self) -> Guard<Arc<Agent>> {
        self.agent.load()
    }

    pub fn id(&self) -> AgentId {
        *self.agent.address()
    }

    pub fn name(&self) -> String {
        self.agent.load().name.to_string()
    }

    /// The context the agent acts in, in `channel` of `session`: its owner's
    /// delegate (issue 255), with its owner's roles as they are now, limited
    /// to `limiting_roles` - its personality's (`Personality::limiting_roles`,
    /// issues 179, 304).
    pub fn security_context(
        &self,
        session: SessionId,
        channel: ChannelId,
        limiting_roles: Option<&[RoleId]>,
    ) -> SecurityContext {
        let roles = self.owner.as_ref().map(|owner| owner.load().roles.clone()).unwrap_or_default();
        self.agent
            .load()
            .security_context(session, channel)
            .with_roles(roles, limiting_roles.map(<[_]>::to_vec))
    }

    /// Renames the agent on behalf of `ctx` (`LiveDocument::update`):
    /// stored first - which is where the permission is checked (issue 11) -
    /// and only then changed live and announced.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `new_name` is no name, otherwise whatever
    /// the store reports; the agent is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: &str) -> StoreResult<()> {
        self.agent.update(ctx, &AgentUpdateEvent::NameChanged(new_name.to_string())).await.map(drop)
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced (`LiveDocument::take_over`).
    pub fn process_global_event(&self, event: &GlobalEvent) {
        self.agent.take_over(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{Personality, PersonalityId};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::store::Persistence;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::{Name, ScopedName};

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
            security_attributes: SecurityAttributes::systems(),
            reasoning_visibility: Visibility::Channel,
            hears_during_turn: Default::default(),
            deleted: None,
        };
        let agent = Agent::new(
            ScopedName::new("assistant", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(UserId::new()),
        );
        let store = Persistence::in_memory();
        let ctx = SecurityContext::default();
        store.entity_stores().unwrap().agents.create(&ctx, &agent).await.unwrap();
        let controller = AgentController::new(
            agent.clone(),
            None,
            Arc::new(()),
            &store.entity_stores().unwrap(),
        );

        controller.update_name(&ctx, "new_name").await.unwrap();

        assert_eq!(controller.name(), "new_name");
        assert_eq!(
            store.entity_stores().unwrap().agents.get(&ctx, agent.id).await.unwrap().name.as_str(),
            "new_name"
        );
    }

    /// Issue 304: an agent acts with its owner's roles as they are now - a
    /// role given or taken reaches it with the owner's controller - limited
    /// to those its personality names.
    #[tokio::test]
    async fn an_agent_acts_with_its_owners_roles_as_they_are_now_and_its_limit() {
        use caelumora_model::event::GlobalEvent;
        use caelumora_model::role::RoleId;
        use caelumora_model::user::{User, UserId, UserUpdateEvent};
        let (staff, hr) = (RoleId::new(), RoleId::new());
        let alice = User {
            id: UserId::new(),
            username: Name::new("alice").unwrap(),
            email: String::new(),
            roles: vec![staff],
            deleted: None,
        };
        let store = Persistence::in_memory();
        let owner = Arc::new(UserController::new(
            alice.clone(),
            None,
            Arc::new(()),
            &store.entity_stores().unwrap(),
            store.state_store(),
            store.history_store(),
        ));
        let personality = Personality { roles: Some(vec![staff]), ..plain() };
        let agent = Agent::new(
            ScopedName::new("assistant", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(alice.id),
        );
        let controller = AgentController::new(
            agent,
            Some(Arc::clone(&owner)),
            Arc::new(()),
            &store.entity_stores().unwrap(),
        );
        let (session, channel) = (Default::default(), Default::default());

        let context = controller.security_context(session, channel, personality.limiting_roles());
        assert_eq!(
            (context.principal_id, context.roles.clone(), context.limiting_roles.clone()),
            (ActorId::User(alice.id), vec![staff], Some(vec![staff]))
        );
        owner.process_global_event(&GlobalEvent::updated::<User>(
            &alice.id,
            &UserUpdateEvent::RolesChanged(vec![staff, hr]),
        ));
        let context = controller.security_context(session, channel, personality.limiting_roles());
        assert_eq!(context.roles, [staff, hr], "her roles as they are now");
        assert!(
            context.has_role(staff) && !context.has_role(hr),
            "but only those its personality names count"
        );
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
            security_attributes: SecurityAttributes::systems(),
            reasoning_visibility: Visibility::Channel,
            hears_during_turn: Default::default(),
            deleted: None,
        }
    }
}
