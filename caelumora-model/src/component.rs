use crate::security::{Secured, SecurityAttributes};
use serde::{Deserialize, Serialize};
use serde_json::Value;


pub const STAGE_PERCEIVE: &str = "perceive";
pub const STAGE_PLAN: &str = "plan";
pub const STAGE_EMIT: &str = "emit";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentConfig {
    #[serde(default)]
    pub component_nodes: Vec<ComponentDescriptor>,
}

impl Default for ComponentConfig {
    fn default() -> Self {
        Self {
            component_nodes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDescriptor {
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
    /// Who may use and change a preconfigured component (issue 11). Only
    /// meaningful for descriptors kept in the `EntityStore`; those inside a
    /// personality are covered by the personality's attributes.
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
}

impl Secured for ComponentDescriptor {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum ComponentFailPolicy {
    #[default]
    FailOpen,
    FailClosed,
}

