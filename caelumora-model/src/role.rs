//! Roles (issues 23, 133, 142): what someone is in the organisation, as the
//! administrator configures it - each role with a stable id, the name it is
//! written by, and the clearances it grants. What is stored names a role by
//! its id (`RoleId`): a user's roles, a personality's limiting roles, an
//! entry of an access list. What is written by hand - the user providers'
//! configuration, a seed - names it by its name, resolved when it is
//! loaded; the `SecurityManager` translates both ways.

use crate::actor::ActorId;
use crate::confidentiality::{Clearance, Clearances, Compartment, Compartments};
use crate::security::SecurityContext;
use crate::store::{EntityStore, StoreResult, StoreResultExt};
use serde::Deserialize;
use std::collections::BTreeMap;
use thiserror::Error;

define_id!(RoleId);

/// The roles the administrator configured, each with the clearances it
/// grants (issues 133, 142). Written as a list of definitions, each an
/// `id`, a `name` and its `clearances` - the `roles` section of the
/// configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<RoleDefinition>")]
pub struct Roles {
    grants: BTreeMap<RoleId, Vec<Clearance>>,
    ids: BTreeMap<String, RoleId>,
}

/// One role as the configuration defines it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDefinition {
    id: RoleId,
    name: String,
    #[serde(default)]
    clearances: Vec<Clearance>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RoleError {
    #[error("A role needs a name")]
    UnnamedRole,
    #[error("Role '{0}' is configured twice")]
    DuplicateRole(String),
    #[error("Role '{0}' is not configured")]
    UnknownRole(String),
    #[error("Role '{role}' grants a clearance for compartment '{compartment}', which is not configured")]
    UnknownCompartment { role: String, compartment: Compartment },
}

impl TryFrom<Vec<RoleDefinition>> for Roles {
    type Error = RoleError;

    fn try_from(definitions: Vec<RoleDefinition>) -> Result<Self, Self::Error> {
        let mut roles = Roles::default();
        for definition in definitions {
            if definition.name.is_empty() {
                return Err(RoleError::UnnamedRole);
            }
            if roles.ids.contains_key(&definition.name) || roles.grants.contains_key(&definition.id) {
                return Err(RoleError::DuplicateRole(definition.name));
            }
            roles.ids.insert(definition.name, definition.id);
            roles.grants.insert(definition.id, definition.clearances);
        }
        Ok(roles)
    }
}

impl Roles {
    /// The roles `definitions` name - an id, a name, the clearances it
    /// grants.
    pub fn new(definitions: impl IntoIterator<Item = (RoleId, String, Vec<Clearance>)>) -> Result<Self, RoleError> {
        Self::try_from(definitions.into_iter().map(|(id, name, clearances)| RoleDefinition { id, name, clearances }).collect::<Vec<_>>())
    }

    /// Nothing if every compartment the roles grant is configured.
    pub fn check(&self, compartments: &Compartments) -> Result<(), RoleError> {
        for (name, id) in &self.ids {
            let clearances = self.grants.get(id).into_iter().flatten();
            for compartment in clearances.filter_map(|c| c.compartment.as_ref()) {
                if !compartments.names().any(|known| known == compartment) {
                    return Err(RoleError::UnknownCompartment { role: name.clone(), compartment: compartment.clone() });
                }
            }
        }
        Ok(())
    }

    /// The id of the role named `name`.
    ///
    /// # Errors
    /// `RoleError::UnknownRole` if no role of that name is configured.
    pub fn id_of(&self, name: &str) -> Result<RoleId, RoleError> {
        self.ids.get(name).copied().ok_or_else(|| RoleError::UnknownRole(name.to_string()))
    }

    /// The ids of the roles `names` names, in their order.
    ///
    /// # Errors
    /// `RoleError::UnknownRole` for the first name that is not configured.
    pub fn ids_of<S: AsRef<str>>(&self, names: &[S]) -> Result<Vec<RoleId>, RoleError> {
        names.iter().map(|name| self.id_of(name.as_ref())).collect()
    }

    /// The name of the role `id`, if it is configured.
    pub fn name_of(&self, id: RoleId) -> Option<&str> {
        self.ids.iter().find(|(_, known)| **known == id).map(|(name, _)| name.as_str())
    }

    /// Everything someone with `roles` is cleared for. A role that is not
    /// configured grants nothing.
    pub fn clearances_of(&self, roles: &[RoleId]) -> Clearances {
        Clearances::new(roles.iter().filter_map(|r| self.grants.get(r)).flatten().cloned().collect())
    }
}

/// The roles of `actor` if it is a user (issues 133, 142), read in the
/// runtime's name: what someone is cleared for decides, it is not shown.
/// None for anyone else - an agent's are its owner's, which its caller
/// asks for - and none for a user that no longer exists. Read from the
/// store each time: roles change at a login, past any copy of the user.
///
/// # Errors
/// Whatever else reading the user reports.
pub async fn user_roles(store: &dyn EntityStore, actor: ActorId) -> StoreResult<Vec<RoleId>> {
    let ActorId::User(id) = actor else { return Ok(Vec::new()) };
    Ok(store.get_user(&SecurityContext::system(), id).await.optional()?.map(|user| user.roles).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::{Classification, Confidentiality::*};

    fn organisation() -> Compartments {
        Compartments::new([(Compartment::new("hr"), vec![])]).unwrap()
    }

    #[test]
    fn roles_grant_clearances_and_name_only_configured_compartments() {
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let roles = Roles::new([
            (hr, "hr".to_string(), vec![Clearance { level: Confidential, compartment: Some(Compartment::new("hr")) }]),
            (staff, "staff".to_string(), vec![Clearance { level: Internal, compartment: None }]),
        ]).unwrap();
        let compartments = organisation();
        assert_eq!(roles.check(&compartments), Ok(()));
        let both = roles.clearances_of(&[hr, staff, RoleId::new()]);
        let hr_only = Classification::new(Confidential, [Compartment::new("hr")]).unwrap();
        assert!(both.admits(&hr_only, &compartments) && both.admits(&Classification::general(Internal), &compartments));
        let typo = Roles::new([(RoleId::new(), "x".to_string(), vec![Clearance { level: Internal, compartment: Some(Compartment::new("hrr")) }])]).unwrap();
        assert!(matches!(typo.check(&compartments), Err(RoleError::UnknownCompartment { .. })));
        let id = RoleId::new();
        let written: Roles = serde_json::from_value(serde_json::json!([
            {"id": id, "name": "hr", "clearances": [{"level": "confidential", "compartment": "hr"}, {"level": "internal"}]}
        ])).unwrap();
        assert!(written.clearances_of(&[id]).admits(&Classification::general(Internal), &compartments));
    }

    #[test]
    fn a_role_is_stored_by_its_id_and_written_by_its_name() {
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let roles = Roles::new([(hr, "hr".to_string(), vec![]), (staff, "staff".to_string(), vec![])]).unwrap();
        assert_eq!(roles.id_of("hr"), Ok(hr));
        assert_eq!(roles.ids_of(&["staff", "hr"]), Ok(vec![staff, hr]));
        assert_eq!(roles.name_of(staff), Some("staff"));
        assert_eq!(roles.id_of("unknown"), Err(RoleError::UnknownRole("unknown".to_string())));
        assert_eq!(roles.name_of(RoleId::new()), None);
        let twice = Roles::new([(hr, "hr".to_string(), vec![]), (hr, "other".to_string(), vec![])]);
        assert_eq!(twice, Err(RoleError::DuplicateRole("other".to_string())), "an id is one role");
        let named_twice = Roles::new([(hr, "hr".to_string(), vec![]), (staff, "hr".to_string(), vec![])]);
        assert_eq!(named_twice, Err(RoleError::DuplicateRole("hr".to_string())), "and so is a name");
    }
}
