use serde::{Deserialize, Serialize};
use serde_json::Value;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentConfig {
    #[serde(default)]
    pub component_nodes: Vec<ComponentNodeConfig>,
    #[serde(default)]
    pub component_bindings: Vec<ComponentBindingConfig>,
}

impl Default for ComponentConfig {
    fn default() -> Self {
        Self {
            component_nodes: Vec::new(),
            component_bindings: Vec::new(),
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
pub struct ComponentBindingConfig {
    pub stage: String,
    #[serde(default)]
    pub components: Vec<String>,
}
