use serde::{Deserialize, Serialize};
use serde_json::Value;


pub const STAGE_PERCEIVE: &str = "perceive";
pub const STAGE_PLAN: &str = "plan";
pub const STAGE_EMIT: &str = "emit";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentConfig {
    #[serde(default)]
    pub component_nodes: Vec<ComponentNodeConfig>,
}

impl Default for ComponentConfig {
    fn default() -> Self {
        Self {
            component_nodes: Vec::new(),
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
    pub tags: Vec<String>,
    #[serde(default)]
    pub fail_policy: ComponentFailPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum ComponentFailPolicy {
    #[default]
    FailOpen,
    FailClosed,
}

