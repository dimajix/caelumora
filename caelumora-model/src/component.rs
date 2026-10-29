//! How components are written down: the descriptors of a personality, a
//! template, a channel or the configuration, and the bundles of them that
//! several include by `kind: "ref"` (issue 270).

use crate::removal::Removal;
use crate::scope::{ScopeField, Scoped};
use crate::security::{Secured, SecurityAttributes};
use crate::store::{EntityKind, NoEntity, Stored};
use crate::value::name::ScopedName;
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

define_id!(ComponentBundleId);

/// A bundle of components (issues 270, 455): a list of descriptors kept as
/// an entity of its own, which a `ComponentConfig` includes by `kind: "ref"`
/// and its name - one component, or several that personalities share -,
/// with who may use and change it (issue 11). Its name is unique in its
/// scope (issue 250).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentBundle {
    pub id: ComponentBundleId,
    /// What a `ref` finds it by: its name and where it lies (issue 177), the
    /// global scope unless named.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What a `ref` to it stands for, in their order.
    pub components: ComponentConfig,
    /// Who may use it - read it, as whoever builds what includes it - and
    /// change it.
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl ComponentBundle {
    /// A new bundle of `components` under `name`, with `security_attributes`.
    pub fn new(
        name: ScopedName,
        components: ComponentConfig,
        security_attributes: SecurityAttributes,
    ) -> Self {
        Self { id: ComponentBundleId::new(), name, components, security_attributes, deleted: None }
    }
}

impl Stored for ComponentBundle {
    const KIND: EntityKind = EntityKind::new("component_bundle");
    type Id = ComponentBundleId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> ComponentBundleId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }
}

impl Secured for ComponentBundle {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for ComponentBundle {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
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
    use crate::scope::Scope;
    use crate::user::UserId;

    #[test]
    fn a_bundle_is_written_as_its_id_its_name_its_list_and_its_attributes() {
        let owner = ActorId::System;
        let user_id = UserId::new();
        let components = ComponentConfig {
            component_nodes: vec![
                ComponentDescriptor::new("search", "web_search"),
                ComponentDescriptor::new("stop", "stop_tool"),
            ],
        };
        let bundle = ComponentBundle::new(
            ScopedName::new("basics", Scope::UserScope { user_id }).unwrap(),
            components.clone(),
            SecurityAttributes::owned_by(owner),
        );
        let json = serde_json::to_value(&bundle).unwrap();
        assert_eq!(json["id"], bundle.id.to_string());
        assert_eq!(json["name"], "basics", "its name and scope flattened, as a definition's are");
        assert!(json["scope"]["UserScope"].is_object(), "{json}");
        assert_eq!(
            json["components"][1]["kind"], "stop_tool",
            "the list directly under its key (issue 201)"
        );
        assert_eq!(json["security_attributes"]["owner_id"], serde_json::to_value(owner).unwrap());

        let read: ComponentBundle = serde_json::from_value(json).unwrap();
        assert_eq!((read.id, read.scoped_name()), (bundle.id, bundle.scoped_name()));
        assert_eq!(read.components, components, "in its order");
        assert_eq!(read.security_attributes.owner_id, owner);
        let global: ComponentBundle = serde_json::from_value(
            serde_json::json!({ "id": bundle.id, "name": "basics", "components": [] }),
        )
        .unwrap();
        assert_eq!(global.name.scope, Scope::GlobalScope, "global unless named");
    }
}
