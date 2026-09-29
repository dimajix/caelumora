use arc_swap::ArcSwap;
use caelumora_model::agent::{Agent, AgentId, AgentUpdateEvent};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StoreResult};
use std::sync::Arc;

/// The one writer of an agent's document.
#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    session_id: SessionId,
    agent: ArcSwap<Agent>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
    /// Held while the agent is changed and stored, like the other
    /// controllers.
    write_lock: tokio::sync::Mutex<()>,
}

impl AgentController {
    /// The live `agent` of an actor in session `session_id`, which its
    /// announcements name.
    pub fn new(agent: Agent, session_id: SessionId, event_publisher: Arc<dyn EventPublisher>, entity_store: Arc<dyn EntityStore>) -> Self {
        Self {
            id: agent.id,
            session_id,
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
        self.agent.load().metadata.name.clone()
    }

    /// Renames the agent on behalf of `ctx`: stored first - which is where
    /// the permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// Whatever the store reports; the agent is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: String) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let event = AgentUpdateEvent::NameChanged(new_name);
        let agent = self.apply(&event);
        self.entity_store.update_agent(ctx, &agent).await?;
        let attributes = agent.security_attributes.clone();
        self.agent.store(Arc::new(agent));
        self.event_publisher.publish(GlobalEvent::AgentUpdated(self.session_id, self.id, event), attributes);
        Ok(())
    }

    /// The agent with `event` applied, not yet stored anywhere.
    fn apply(&self, event: &AgentUpdateEvent) -> Agent {
        let mut agent = (**self.agent.load()).clone();
        match event {
            AgentUpdateEvent::NameChanged(name) => agent.metadata.name = name.clone(),
        }
        agent
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        if let GlobalEvent::AgentUpdated(_, aid, update) = event {
            if *aid == self.id {
                self.agent.store(Arc::new(self.apply(update)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{Personality, PersonalityId};
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::store_impl::memory::MemoryStore;

    #[tokio::test]
    async fn a_renamed_agent_is_stored_first() {
        let personality = Personality {
            id: PersonalityId::new(),
            metadata: Metadata::default(),
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_confidentiality: caelumora_model::confidentiality::Confidentiality::Public,
        };
        let agent = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality, caelumora_model::actor::ActorId::None);
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.create_agent(&ctx, &agent).await.unwrap();
        let controller = AgentController::new(agent.clone(), SessionId::new(), Arc::new(()), store.clone());

        controller.update_name(&ctx, "new_name".to_string()).await.unwrap();

        assert_eq!(controller.name(), "new_name");
        assert_eq!(store.get_agent(&ctx, agent.id).await.unwrap().metadata.name, "new_name");
    }
}
