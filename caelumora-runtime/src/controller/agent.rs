use caelumora_model::name::Name;
use arc_swap::ArcSwap;
use caelumora_model::agent::{Agent, AgentId, AgentUpdateEvent};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StoreResult};
use std::sync::Arc;

/// The one writer of an agent's document in this instance, shared by all
/// the agent's actors in all sessions (issue 121).
#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    agent: ArcSwap<Agent>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
    /// Held while the agent is changed and stored, like the other
    /// controllers.
    write_lock: tokio::sync::Mutex<()>,
}

impl AgentController {
    /// The live `agent`. Made by the `ControllerManager`, which keeps one
    /// per agent.
    pub(crate) fn new(agent: Agent, event_publisher: Arc<dyn EventPublisher>, entity_store: Arc<dyn EntityStore>) -> Self {
        Self {
            id: agent.id,
            agent: ArcSwap::from_pointee(agent),
            event_publisher,
            entity_store,
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

    /// Renames the agent on behalf of `ctx`: stored first - which is where
    /// the permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// Whatever the store reports; the agent is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: Name) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let event = AgentUpdateEvent::NameChanged(new_name);
        let agent = self.apply(&event);
        self.entity_store.update_agent(ctx, &agent).await?;
        let attributes = agent.security_attributes.clone();
        self.agent.store(Arc::new(agent));
        self.event_publisher.publish(GlobalEvent::AgentUpdated(self.id, event), attributes);
        Ok(())
    }

    /// The agent with `event` applied, not yet stored anywhere.
    fn apply(&self, event: &AgentUpdateEvent) -> Agent {
        let mut agent = (**self.agent.load()).clone();
        match event {
            AgentUpdateEvent::NameChanged(name) => agent.name.name = name.clone(),
        }
        agent
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced. An event carries the resulting value, so taking over
    /// this controller's own change again changes nothing.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        if let GlobalEvent::AgentUpdated(aid, update) = event {
            if *aid == self.id {
                self.agent.store(Arc::new(self.apply(update)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::{Name, ScopedName};
    use caelumora_model::agent::{Personality, PersonalityId};
    use caelumora_model::scope::Scope;
    use caelumora_model::store_impl::memory::MemoryStore;

    #[tokio::test]
    async fn a_renamed_agent_is_stored_first() {
        let personality = Personality {
            description: String::new(),
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
        };
        let agent = Agent::new(ScopedName::new("assistant", Scope::GlobalScope).unwrap(), &personality, caelumora_model::actor::ActorId::None);
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.create_agent(&ctx, &agent).await.unwrap();
        let controller = AgentController::new(agent.clone(), Arc::new(()), store.clone());

        controller.update_name(&ctx, Name::new("new_name").unwrap()).await.unwrap();

        assert_eq!(controller.name(), "new_name");
        assert_eq!(store.get_agent(&ctx, agent.id).await.unwrap().name.as_str(), "new_name");
    }
}
