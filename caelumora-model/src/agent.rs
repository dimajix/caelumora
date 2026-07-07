use crate::actor::{ActorId, Envelope, EventContext};
use crate::approval::ApprovableToolCall;
use crate::component::ComponentConfig;
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::{Message, MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};
use crate::security::{SecurityAttributes, SecurityContext};
use crate::session::SessionId;
use crate::store::EntityStore;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    session_id: SessionId,
    agent: ArcSwap<Agent>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
}

impl AgentController {
    pub fn new(
        session_id: SessionId,
        agent: Agent,
        event_publisher: Arc<dyn EventPublisher>,
        entity_store: Arc<dyn EntityStore>
    ) -> Self {
        Self {
            id: agent.id,
            session_id,
            agent: ArcSwap::from_pointee(agent),
            event_publisher,
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
        self.event_publisher.publish(GlobalEvent::AgentUpdated(self.session_id, self.id, event));
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
            GlobalEvent::AgentUpdated(_, aid, update) if *aid == self.id => {
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessingState {
    #[default]
    Idle,
    Active,
    Cancelling,
    Sleep,
    Silence,
    WaitingForApproval,
}

impl From<u8> for ProcessingState {
    fn from(v: u8) -> Self {
        match v {
            0 => ProcessingState::Idle,
            1 => ProcessingState::Active,
            2 => ProcessingState::Cancelling,
            3 => ProcessingState::Sleep,
            4 => ProcessingState::Silence,
            5 => ProcessingState::WaitingForApproval,
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
            ProcessingState::Sleep => 3,
            ProcessingState::Silence => 4,
            ProcessingState::WaitingForApproval => 5,
        }
    }
}

// We use this helper to bridge u8 <-> ProcessingState during Serde operations
mod processing_state_serde {
    use super::*;
    use std::sync::atomic::Ordering;

    // Serialization: Arc<AtomicU8> -> ProcessingState (String/Enum)
    pub fn serialize<S>(val: &Arc<AtomicU8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let state = ProcessingState::from(val.load(Ordering::SeqCst));
        state.serialize(serializer)
    }

    // Deserialization: ProcessingState (String/Enum) -> Arc<AtomicU8>
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<AtomicU8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let state: ProcessingState = Deserialize::deserialize(deserializer)?;
        Ok(Arc::new(AtomicU8::new(u8::from(state))))
    }
}

fn default_processing_state() -> Arc<AtomicU8> {
    Arc::new(AtomicU8::new(ProcessingState::Idle as u8))
}


define_id!(AgentId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub metadata: Metadata,
    pub personality_id: PersonalityId,
    pub security_attributes: SecurityAttributes,
    pub security_context: SecurityContext,
}

impl Agent {
    pub fn new(name:String, scope:Scope, personality: &Personality, mut security_context: SecurityContext) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        security_context.caller_id = ActorId::Agent(agent_id);
        let security_attributes = SecurityAttributes {
            owner_id: ActorId::Agent(agent_id),
        };
        Agent {
            id: agent_id,
            personality_id: personality.id,
            metadata: Metadata {
                name,
                scope,
                description: personality.metadata.description.clone(),
                ..Metadata::default()
            },
            security_attributes,
            security_context,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentChannelState {
    #[serde(
        default = "default_processing_state",
        serialize_with = "processing_state_serde::serialize",
        deserialize_with = "processing_state_serde::deserialize"
    )]
    pub processing_state: Arc<AtomicU8>,
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
    pub context: MessageContext,
    /// Pending tool calls
    #[serde(default)]
    pub pending_tool_calls: Vec<ApprovableToolCall>,
    /// Pending messages while waiting for approval
    #[serde(default)]
    pub pending_events: Vec<Envelope>,
    #[serde(default)]
    pub turn_scratch: HashMap<String, Value>,
    #[serde(default)]
    pub turn_context: Vec<Message>,
    #[serde(default)]
    pub turn_messages: Vec<Message>,
    #[serde(default)]
    pub turn_security_context: SecurityContext,
    #[serde(default)]
    pub turn_event_context: EventContext
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    // Add session specific data of an individual agent from multiple channels here if needed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_processing_state_conversion() {
        assert_eq!(u8::from(ProcessingState::Idle), 0);
        assert_eq!(u8::from(ProcessingState::Active), 1);
        assert_eq!(u8::from(ProcessingState::WaitingForApproval), 5);
        assert_eq!(ProcessingState::from(5), ProcessingState::WaitingForApproval);
    }

    #[test]
    fn test_agent_channel_state_serialization() {
        let state = AgentChannelState::default();
        state.processing_state.store(ProcessingState::WaitingForApproval.into(), Ordering::SeqCst);
        
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"WaitingForApproval\""));
        
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(ProcessingState::from(decoded.processing_state.load(Ordering::SeqCst)), ProcessingState::WaitingForApproval);
    }
}
