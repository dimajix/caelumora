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

/// A preconfigured component: a descriptor kept in the `ComponentStore`
/// under its id, which personalities and templates refer to by `kind:
/// "ref"`, with who may use and change it (issue 11). Stored as the
/// descriptor with its attributes beside its fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredComponent {
    #[serde(flatten)]
    pub descriptor: ComponentDescriptor,
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
}

impl StoredComponent {
    /// `descriptor`, kept with `security_attributes`.
    pub fn new(descriptor: ComponentDescriptor, security_attributes: SecurityAttributes) -> Self {
        Self { descriptor, security_attributes }
    }

    /// What it is kept under.
    pub fn id(&self) -> &str {
        &self.descriptor.id
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
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
    fn a_stored_component_is_written_as_the_descriptor_with_its_attributes_beside_its_fields() {
        let owner = ActorId::System;
        let stored = StoredComponent::new(ComponentDescriptor::new("search", "web_search"), SecurityAttributes::owned_by(owner));
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(json["id"], "search");
        assert_eq!(json["kind"], "web_search");
        assert_eq!(json["security_attributes"]["owner_id"], serde_json::to_value(owner).unwrap(), "as before the store of its own");

        let read: StoredComponent = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(read.id(), "search");
        assert_eq!(read.security_attributes.owner_id, owner);
        let descriptor: ComponentDescriptor = serde_json::from_value(json).unwrap();
        assert_eq!(descriptor.kind, "web_search", "a descriptor with attributes written into it still reads");
    }
}
