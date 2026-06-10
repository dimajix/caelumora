use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::{MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};
use serde_json::Value;


#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PersonalityId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    pub metadata: Metadata,
    #[serde(default)]
    pub pipeline: PipelineConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PipelineStage {
    PerceiveIn,
    HistoryWrite,
    ContextBuild,
    PlanPre,
    EmitOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default)]
    pub component_nodes: Vec<ComponentNodeConfig>,
    #[serde(default)]
    pub pipeline_bindings: Vec<PipelineBindingConfig>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            component_nodes: Vec::new(),
            pipeline_bindings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentNodeConfig {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub fail_policy: ComponentFailPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum ComponentFailPolicy {
    #[default]
    FailOpen,
    FailClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineBindingConfig {
    pub stage: PipelineStage,
    #[serde(default)]
    pub components: Vec<String>,
}


#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub Uuid);

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
