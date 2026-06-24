use crate::component::ComponentConfig;
use crate::event::{EventSender, GlobalEvent};
use crate::message::{MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};
use crate::security::SecurityContext;
use crate::store::EntityStore;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    agent: ArcSwap<Agent>,
    event_sender: Arc<dyn EventSender<AgentUpdateEvent>>,
    entity_store: Arc<dyn EntityStore>,
}

impl AgentController {
    pub fn new(
        agent: Agent,
        event_sender: Arc<dyn EventSender<AgentUpdateEvent>>,
        entity_store: Arc<dyn EntityStore>
    ) -> Self {
        Self {
            id: agent.id,
            agent: ArcSwap::from_pointee(agent),
            event_sender,
            entity_store,
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

    pub async fn update_name(&self, new_name: String) {
        let event = AgentUpdateEvent::NameChanged(new_name);
        self.process_update(&event).await;
        self.event_sender.send(event);
    }

    async fn process_update(&self, event: &AgentUpdateEvent) {
        match event {
            AgentUpdateEvent::NameChanged(name) => {
                let mut agent = (**self.agent.load()).clone();
                agent.metadata.name = name.clone();
                let agent = Arc::new(agent);
                self.agent.store(agent.clone());
                let ctx = SecurityContext::default();
                self.entity_store.update_agent(&ctx, &agent).await.expect("failed to update agent");
            }
        }
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::AgentUpdated(_, _, aid, update) if *aid == self.id => {
                self.process_update(update).await;
            }
            _ => {}
        }
    }
}

define_id!(PersonalityId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    pub metadata: Metadata,
    #[serde(default)]
    pub components: ComponentConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessingState {
    Idle,
    Active,
    Cancelling,
}

impl Default for ProcessingState {
    fn default() -> Self {
        Self::Idle
    }
}

impl From<u8> for ProcessingState {
    fn from(v: u8) -> Self {
        match v {
            0 => ProcessingState::Idle,
            1 => ProcessingState::Active,
            2 => ProcessingState::Cancelling,
            _ => ProcessingState::Idle,
        }
    }
}

impl From<ProcessingState> for u8 {
    fn from(s: ProcessingState) -> Self {
        match s {
            ProcessingState::Idle => 0,
            ProcessingState::Active => 1,
            ProcessingState::Cancelling => 2,
        }
    }
}

define_id!(AgentId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub metadata: Metadata,
    pub personality_id: PersonalityId,
}

impl Agent {
    pub fn new(name:String, scope:Scope, personality: &Personality) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        Agent {
            id: agent_id,
            personality_id: personality.id,
            metadata: Metadata {
                name,
                scope,
                description: personality.metadata.description.clone(),
                ..Metadata::default()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
    pub context: MessageContext,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            history: MessageHistory::new(),
            context: MessageContext::new(),
        }
    }
}
