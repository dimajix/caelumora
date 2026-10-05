use crate::security::{Secured, SecurityAttributes};
use serde::{Deserialize, Serialize};
use serde_json::Value;


/// The components of a personality, a template or the configuration -
/// written by hand (configuration, seed files), so the fields with a
/// default may be left out there (issue 108). Written as the list of its
/// descriptors, directly under the key that holds it (issue 201).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentConfig {
    pub component_nodes: Vec<ComponentDescriptor>,
}

/// One component, written by hand like its `ComponentConfig`: the fields
/// with a default may be left out (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub params: Value,
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

impl ComponentDescriptor {
    /// The component `id` of `kind`, with everything else as it is when
    /// left out by hand.
    pub fn new(id: &str, kind: &str) -> Self {
        Self {
            id: id.to_string(),
            kind: kind.to_string(),
            params: Value::Null,
            tags: Vec::new(),
            fail_policy: Default::default(),
            security_attributes: Default::default(),
        }
    }
}

/// Deleted for good, not marked (issue 164): it is reached only through the
/// personalities and templates that refer to it, and hidden with them.
impl crate::removal::Stored for ComponentDescriptor {}

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

