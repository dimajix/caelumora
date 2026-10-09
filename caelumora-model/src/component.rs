use crate::security::{Secured, SecurityAttributes};
use serde::{Deserialize, Serialize};
use serde_json::Value;


/// The components of a personality, a template or the configuration -
/// written by hand (configuration, seed files), so the fields with a
/// default may be left out there (issue 108). Written as the list of its
/// descriptors, directly under the key that holds it (issue 201).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentConfig {
    pub component_nodes: Vec<ComponentDescriptor>,
}

/// One component, written by hand like its `ComponentConfig`: the fields
/// with a default may be left out (issue 108).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub fail_policy: ComponentFailPolicy,
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
        }
    }
}

/// Preconfigured components: a list of descriptors kept in the
/// `ComponentStore` under an id of its own, which a `ComponentConfig`
/// includes by `kind: "ref"` - one component, or a bundle of them that
/// several personalities share (issue 270) -, with who may use and change
/// it (issue 11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredComponent {
    pub id: String,
    pub components: ComponentConfig,
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
}

impl StoredComponent {
    /// `components`, kept under `id` with `security_attributes`.
    pub fn new(id: &str, components: ComponentConfig, security_attributes: SecurityAttributes) -> Self {
        Self { id: id.to_string(), components, security_attributes }
    }

    /// What it is kept under.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Deleted for good, not marked (issue 164): it is runtime configuration,
/// reached only through the personalities and templates that refer to it,
/// and hidden with them.
impl crate::removal::Stored for StoredComponent {}

impl Secured for StoredComponent {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ComponentFailPolicy {
    #[default]
    FailOpen,
    FailClosed,
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;

    #[test]
    fn stored_components_are_written_as_their_id_their_list_and_their_attributes() {
        let owner = ActorId::System;
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("search", "web_search"), ComponentDescriptor::new("stop", "stop_tool")] };
        let stored = StoredComponent::new("basics", components.clone(), SecurityAttributes::owned_by(owner));
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(json["id"], "basics");
        assert_eq!(json["components"][1]["kind"], "stop_tool", "the list directly under its key (issue 201)");
        assert_eq!(json["security_attributes"]["owner_id"], serde_json::to_value(owner).unwrap());

        let read: StoredComponent = serde_json::from_value(json).unwrap();
        assert_eq!(read.id(), "basics");
        assert_eq!(read.components, components, "in its order");
        assert_eq!(read.security_attributes.owner_id, owner);
    }
}
