use crate::actor::{ActorId, ActorState, EventContext, ProcessingStateHolder};
use crate::component::ComponentConfig;
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::{Message, MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};
use crate::security::{SecurityAttributes, SecurityContext};
use crate::session::SessionId;
use crate::store::EntityStore;
use crate::tool::ToolCallState;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};
use serde_json::Value;
use std::collections::HashMap;
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
        default = "crate::actor::default_processing_state",
        serialize_with = "crate::actor::processing_state_serde::serialize",
        deserialize_with = "crate::actor::processing_state_serde::deserialize"
    )]
    pub processing_state: Arc<ProcessingStateHolder>,
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
    pub context: MessageContext,
    /// Pending tool calls
    #[serde(default)]
    pub tool_calls: Vec<ToolCallState>,
    /// Set by a tool (e.g. stop/bye/silence) or by the run loop itself (on a
    /// detected cancellation) to override what state `Reflecting` lands on
    /// instead of the default `Idle`. Consumed once, at the end of the turn.
    #[serde(default)]
    pub requested_state: Option<ActorState>,
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
    use crate::actor::ActorState;

    #[test]
    fn test_agent_channel_state_serialization() {
        let state = AgentChannelState::default();
        state.processing_state.store(ActorState::WaitingForApproval.into());
        
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"WaitingForApproval\""));
        
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(ActorState::from(decoded.processing_state.load()), ActorState::WaitingForApproval);
    }
}
