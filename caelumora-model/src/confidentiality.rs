//! Classification (issue 133): how sensitive a piece of information is, and
//! for whom it is meant.
//!
//! A `Classification` is a level from a fixed scale and an audience of
//! compartments - areas such as a topic, a group or a customer, which the
//! administrator configures (`Compartments`). What may flow where is decided
//! by the one flow rule, `Classification::flows_to`, from the
//! classifications involved alone - not by who takes part in a channel at
//! the moment, so that a decision stays the same when someone joins or
//! leaves. The `SecurityManager` makes the decisions; the rule is here.
//!
//! Whether anyone but an actor's owner sees something at all is a separate
//! axis, `Visibility`: ownership, not sensitivity.

use crate::channel::ChannelId;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use thiserror::Error;

/// How sensitive information is, from `Public` to `StrictlyConfidential`.
/// Ordered: a higher level is more confidential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidentiality {
    /// Anyone may read it.
    Public,
    /// For the organisation.
    Internal,
    Confidential,
    StrictlyConfidential,
}

impl fmt::Display for Confidentiality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Confidentiality::Public => "public",
            Confidentiality::Internal => "internal",
            Confidentiality::Confidential => "confidential",
            Confidentiality::StrictlyConfidential => "strictly_confidential",
        })
    }
}

/// An area information may belong to - a topic, a group, a customer - by
/// its name. Which compartments exist is configured (`Compartments`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Compartment(String);

impl Compartment {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Compartment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClassificationError {
    #[error("A public classification takes no compartment")]
    PublicWithCompartments,
    #[error("Compartment '{0}' is not configured")]
    UnknownCompartment(Compartment),
    #[error("A compartment needs a name")]
    UnnamedCompartment,
    #[error("Compartment '{0}' is configured twice")]
    DuplicateCompartment(Compartment),
}

/// A level and an audience of compartments (issue 133).
///
/// The compartments are who may read it: adding one widens the audience. A
/// classification without compartments is *general* - readable by everyone
/// of a sufficient level. `Public` takes no compartment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ClassificationFields")]
pub struct Classification {
    level: Confidentiality,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    compartments: BTreeSet<Compartment>,
}

/// A classification as it is written down, checked by `Classification::new`
/// when it is read.
#[derive(Deserialize)]
struct ClassificationFields {
    level: Confidentiality,
    #[serde(default)]
    compartments: BTreeSet<Compartment>,
}

impl TryFrom<ClassificationFields> for Classification {
    type Error = ClassificationError;

    fn try_from(fields: ClassificationFields) -> Result<Self, Self::Error> {
        Self::new(fields.level, fields.compartments)
    }
}

impl Classification {
    /// The classification of a channel whose creator or template names none.
    pub const CHANNEL_DEFAULT: Classification = Classification::general(Confidentiality::Internal);

    /// `level` with the audience `compartments`; refused for `Public` with
    /// compartments. Whether the compartments exist is the configuration's
    /// question (`Compartments::check`).
    pub fn new(level: Confidentiality, compartments: impl IntoIterator<Item = Compartment>) -> Result<Self, ClassificationError> {
        let compartments: BTreeSet<Compartment> = compartments.into_iter().collect();
        if level == Confidentiality::Public && !compartments.is_empty() {
            return Err(ClassificationError::PublicWithCompartments);
        }
        Ok(Self { level, compartments })
    }

    /// `level` without compartments.
    pub const fn general(level: Confidentiality) -> Self {
        Self { level, compartments: BTreeSet::new() }
    }

    pub fn level(&self) -> Confidentiality {
        self.level
    }

    pub fn compartments(&self) -> &BTreeSet<Compartment> {
        &self.compartments
    }

    /// Whether information classified `self` may flow into what is
    /// classified `destination` - a part into a channel, one channel's
    /// contents into another, a result into an agent's context (issue 133).
    ///
    /// The destination's level must be at least this one. If this has
    /// compartments, so must the destination, and every one of them must be
    /// covered by one of these - the same, or one it may read: everything
    /// the destination's audiences see must be meant for them. What is
    /// general flows into any destination of a sufficient level; what has
    /// an audience never into a general destination.
    pub fn flows_to(&self, destination: &Classification, compartments: &Compartments) -> bool {
        if destination.level < self.level {
            return false;
        }
        if self.compartments.is_empty() {
            return true;
        }
        !destination.compartments.is_empty()
            && destination.compartments.iter().all(|reader| self.compartments.iter().any(|source| compartments.may_read(reader, source)))
    }
}

impl Classification {
    /// This classification changed as little as it must to flow into
    /// `destination`, whatever compartments read which: its level at most
    /// the destination's, and the destination's compartments added to its
    /// audience - none if it is general already, or the destination is
    /// general, or it becomes public. What flows already stays as it is, up
    /// to the audiences added, which may read it anyway. What a channel's messages become when the channel is
    /// released to `destination` (issue 133).
    pub fn fitted_into(&self, destination: &Classification) -> Classification {
        let level = self.level.min(destination.level);
        let general = self.compartments.is_empty() || destination.compartments.is_empty() || level == Confidentiality::Public;
        let compartments = if general {
            BTreeSet::new()
        } else {
            self.compartments.union(&destination.compartments).cloned().collect()
        };
        Classification { level, compartments }
    }
}

impl Default for Classification {
    fn default() -> Self {
        Self::CHANNEL_DEFAULT
    }
}

impl fmt::Display for Classification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.level)?;
        if !self.compartments.is_empty() {
            let names: Vec<&str> = self.compartments.iter().map(Compartment::name).collect();
            write!(f, " {{{}}}", names.join(", "))?;
        }
        Ok(())
    }
}

/// The compartments the administrator configured, and which may read which
/// (issue 133). The read relation is closed transitively when it is built:
/// what a compartment wrote may contain what it read, so whoever may read
/// the one reads the other too. Every compartment reads itself.
///
/// Written as a list of definitions, each a `name` and the compartments it
/// `reads` - the `compartments` section of the configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<CompartmentDefinition>")]
pub struct Compartments {
    reads: BTreeMap<Compartment, BTreeSet<Compartment>>,
}

/// One compartment as the configuration defines it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompartmentDefinition {
    name: Compartment,
    #[serde(default)]
    reads: Vec<Compartment>,
}

impl TryFrom<Vec<CompartmentDefinition>> for Compartments {
    type Error = ClassificationError;

    fn try_from(definitions: Vec<CompartmentDefinition>) -> Result<Self, Self::Error> {
        Self::new(definitions.into_iter().map(|d| (d.name, d.reads)))
    }
}

impl Compartments {
    /// The compartments `definitions` name, each with the compartments it
    /// may read. Refused if a name is empty, given twice, or reads a
    /// compartment that is not defined.
    pub fn new(definitions: impl IntoIterator<Item = (Compartment, Vec<Compartment>)>) -> Result<Self, ClassificationError> {
        let mut reads: BTreeMap<Compartment, BTreeSet<Compartment>> = BTreeMap::new();
        for (compartment, readable) in definitions {
            if compartment.name().is_empty() {
                return Err(ClassificationError::UnnamedCompartment);
            }
            if reads.contains_key(&compartment) {
                return Err(ClassificationError::DuplicateCompartment(compartment));
            }
            let mut readable: BTreeSet<Compartment> = readable.into_iter().collect();
            readable.insert(compartment.clone());
            reads.insert(compartment, readable);
        }
        if let Some(unknown) = reads.values().flatten().find(|c| !reads.contains_key(*c)) {
            return Err(ClassificationError::UnknownCompartment(unknown.clone()));
        }
        // Transitive closure: add what the readable ones read until nothing
        // changes.
        loop {
            let mut changed = false;
            let snapshot = reads.clone();
            for readable in reads.values_mut() {
                let further: BTreeSet<Compartment> = readable.iter().flat_map(|c| snapshot[c].iter().cloned()).collect();
                if !further.is_subset(readable) {
                    readable.extend(further);
                    changed = true;
                }
            }
            if !changed {
                return Ok(Self { reads });
            }
        }
    }

    /// The configured compartments, by name.
    pub fn names(&self) -> impl Iterator<Item = &Compartment> {
        self.reads.keys()
    }

    /// Whether `reader`'s audience may read what is meant for `source`:
    /// the same compartment, or one it reads, directly or through others.
    pub fn may_read(&self, reader: &Compartment, source: &Compartment) -> bool {
        reader == source || self.reads.get(reader).is_some_and(|readable| readable.contains(source))
    }

    /// Nothing if every compartment of `classification` is configured.
    pub fn check(&self, classification: &Classification) -> Result<(), ClassificationError> {
        match classification.compartments.iter().find(|c| !self.reads.contains_key(*c)) {
            Some(unknown) => Err(ClassificationError::UnknownCompartment(unknown.clone())),
            None => Ok(()),
        }
    }
}

/// What someone may see (issue 133): what is classified up to `level`, in
/// `compartment` - or, without one, what is general. A clearance is on the
/// side of who reads: a person *belongs* to a compartment, while a channel
/// is *published* to it; so a clearance is judged by its own rule
/// (`Clearances::admits`), not by the flow rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    pub level: Confidentiality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compartment: Option<Compartment>,
}

impl Clearance {
    /// Whether this clearance sees what is classified `classification`: at
    /// most its level, and - if it has compartments - through a
    /// compartment of this clearance that reads one of them; if it has
    /// none, only a clearance without compartment sees it.
    fn sees(&self, classification: &Classification, compartments: &Compartments) -> bool {
        if classification.level > self.level {
            return false;
        }
        match &self.compartment {
            None => classification.compartments.is_empty(),
            Some(own) => classification.compartments.iter().any(|c| compartments.may_read(own, c)),
        }
    }
}

/// Everything someone is cleared for: the clearances of all their roles
/// (issue 133). Empty: only what is public.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clearances(Vec<Clearance>);

impl Clearances {
    /// Whether what is classified `classification` may be seen - by one of
    /// the clearances, or because it is public.
    pub fn admits(&self, classification: &Classification, compartments: &Compartments) -> bool {
        classification.level == Confidentiality::Public || self.0.iter().any(|c| c.sees(classification, compartments))
    }
}

/// The roles the administrator configured, each with the clearances it
/// grants (issue 133, [142]). Written as a list of definitions, each a
/// `name` and its `clearances` - the `roles` section of the configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<RoleDefinition>")]
pub struct Roles {
    grants: BTreeMap<String, Vec<Clearance>>,
}

/// One role as the configuration defines it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDefinition {
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
        let mut grants = BTreeMap::new();
        for definition in definitions {
            if definition.name.is_empty() {
                return Err(RoleError::UnnamedRole);
            }
            if grants.contains_key(&definition.name) {
                return Err(RoleError::DuplicateRole(definition.name));
            }
            grants.insert(definition.name, definition.clearances);
        }
        Ok(Self { grants })
    }
}

impl Roles {
    /// The roles `definitions` name, each with the clearances it grants.
    pub fn new(definitions: impl IntoIterator<Item = (String, Vec<Clearance>)>) -> Result<Self, RoleError> {
        Self::try_from(definitions.into_iter().map(|(name, clearances)| RoleDefinition { name, clearances }).collect::<Vec<_>>())
    }

    /// Nothing if every compartment the roles grant is configured.
    pub fn check(&self, compartments: &Compartments) -> Result<(), RoleError> {
        for (role, clearances) in &self.grants {
            for compartment in clearances.iter().filter_map(|c| c.compartment.as_ref()) {
                if !compartments.reads.contains_key(compartment) {
                    return Err(RoleError::UnknownCompartment { role: role.clone(), compartment: compartment.clone() });
                }
            }
        }
        Ok(())
    }

    /// Nothing if `role` is configured.
    pub fn check_role(&self, role: &str) -> Result<(), RoleError> {
        if self.grants.contains_key(role) { Ok(()) } else { Err(RoleError::UnknownRole(role.to_string())) }
    }

    /// Everything someone with `roles` is cleared for. A role that is not
    /// configured grants nothing.
    pub fn clearances_of(&self, roles: &[String]) -> Clearances {
        Clearances(roles.iter().filter_map(|r| self.grants.get(r)).flatten().cloned().collect())
    }
}

/// Who sees a part of what an actor produces - an ownership flag beside the
/// classification (issue 133). `Owner` is what never reaches a channel: the
/// actor sees it, and its owner where they may see what the actor saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Published to the channel, which may read it as it reads the channel.
    #[default]
    Channel,
    /// Only the actor and its owner.
    Owner,
}

impl Visibility {
    pub fn is_channel(&self) -> bool {
        *self == Visibility::Channel
    }
}

/// A channel as the confidentiality rules see it: where it is, and the
/// classification of its contents.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChannelClassification {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub classification: Classification,
}

#[cfg(test)]
mod tests {
    use super::*;
    use Confidentiality::*;

    fn compartment(name: &str) -> Compartment {
        Compartment::new(name)
    }

    fn compartment_named(name: &str) -> Compartment {
        compartment(name)
    }

    fn classified(level: Confidentiality, compartments: &[&str]) -> Classification {
        Classification::new(level, compartments.iter().map(|c| compartment(c))).unwrap()
    }

    /// HR management reads HR, HR reads finance.
    fn organisation() -> Compartments {
        Compartments::new([
            (compartment("hr_management"), vec![compartment("hr")]),
            (compartment("hr"), vec![compartment("finance")]),
            (compartment("finance"), vec![]),
            (compartment("sales"), vec![]),
        ])
        .unwrap()
    }

    #[test]
    fn the_levels_are_ordered_by_sensitivity() {
        assert!(Public < Internal && Internal < Confidential && Confidential < StrictlyConfidential);
        assert_eq!(serde_json::to_value(StrictlyConfidential).unwrap(), "strictly_confidential");
        assert_eq!(Classification::default(), Classification::general(Internal));
    }

    #[test]
    fn public_takes_no_compartment() {
        assert_eq!(Classification::new(Public, [compartment("hr")]), Err(ClassificationError::PublicWithCompartments));
        let written = serde_json::json!({"level": "public", "compartments": ["hr"]});
        assert!(serde_json::from_value::<Classification>(written).is_err(), "not even when it is read");
    }

    #[test]
    fn a_classification_is_written_without_an_empty_audience() {
        assert_eq!(serde_json::to_value(Classification::general(Internal)).unwrap(), serde_json::json!({"level": "internal"}));
        let hr = classified(Confidential, &["hr"]);
        assert_eq!(serde_json::to_value(&hr).unwrap(), serde_json::json!({"level": "confidential", "compartments": ["hr"]}));
        assert_eq!(serde_json::from_value::<Classification>(serde_json::json!({"level": "confidential", "compartments": ["hr"]})).unwrap(), hr);
    }

    #[test]
    fn a_classification_is_fitted_into_a_channel_by_as_little_as_must_change() {
        let compartments = organisation();
        let fitted = |source: Classification, channel: Classification| {
            let fitted = source.fitted_into(&channel);
            assert!(fitted.flows_to(&channel, &compartments), "{fitted} flows into {channel}");
            fitted
        };
        let hr_confidential = classified(Confidential, &["hr"]);
        assert_eq!(fitted(hr_confidential.clone(), classified(Internal, &["hr"])), classified(Internal, &["hr"]), "the level lowered");
        assert_eq!(fitted(classified(Confidential, &["hr", "sales"]), classified(Internal, &["hr"])), classified(Internal, &["hr", "sales"]), "keeps its wider audience");
        assert_eq!(fitted(classified(Internal, &["hr"]), classified(Internal, &["hr", "sales"])), classified(Internal, &["hr", "sales"]), "released to the new audience");
        assert_eq!(fitted(hr_confidential.clone(), Classification::general(Internal)), Classification::general(Internal), "released to everyone");
        assert_eq!(fitted(hr_confidential.clone(), Classification::general(Public)), Classification::general(Public));
        assert_eq!(fitted(classified(Internal, &["hr"]), classified(StrictlyConfidential, &["hr"])), classified(Internal, &["hr"]), "raising changes nothing");
        assert_eq!(fitted(Classification::general(Internal), classified(Internal, &["hr"])), Classification::general(Internal), "what is general stays general");
    }

    #[test]
    fn a_clearance_sees_per_compartment_up_to_its_own_level() {
        let compartments = organisation();
        let clearance = |level, compartment: Option<&str>| Clearance { level, compartment: compartment.map(compartment_named) };
        // Alice: internal of sales, confidential of HR, and what is general up to internal.
        let alice = Clearances(vec![
            clearance(Internal, Some("sales")),
            clearance(Confidential, Some("hr")),
            clearance(Internal, None),
        ]);
        assert!(alice.admits(&classified(Confidential, &["hr"]), &compartments));
        assert!(!alice.admits(&classified(Confidential, &["sales"]), &compartments), "only internal of sales");
        assert!(alice.admits(&classified(Internal, &["sales"]), &compartments));
        assert!(alice.admits(&classified(Confidential, &["finance"]), &compartments), "HR reads finance");
        assert!(alice.admits(&classified(Confidential, &["hr", "sales"]), &compartments), "published to HR too");
        assert!(!alice.admits(&classified(Internal, &["hr_management"]), &compartments), "not hers");
        assert!(alice.admits(&Classification::general(Internal), &compartments));
        assert!(!alice.admits(&Classification::general(Confidential), &compartments), "a compartment says nothing of what is general");
        assert!(Clearances::default().admits(&Classification::general(Public), &compartments), "public is public");
        assert!(!Clearances::default().admits(&Classification::general(Internal), &compartments), "without a clearance only that");
        let management = Clearances(vec![clearance(Internal, Some("hr_management"))]);
        assert!(management.admits(&classified(Internal, &["finance"]), &compartments), "through what it reads");
    }

    #[test]
    fn roles_grant_clearances_and_name_only_configured_compartments() {
        let roles = Roles::new([
            ("hr".to_string(), vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }]),
            ("staff".to_string(), vec![Clearance { level: Internal, compartment: None }]),
        ]).unwrap();
        let compartments = organisation();
        assert_eq!(roles.check(&compartments), Ok(()));
        let both = roles.clearances_of(&["hr".to_string(), "staff".to_string(), "unknown".to_string()]);
        assert!(both.admits(&classified(Confidential, &["hr"]), &compartments) && both.admits(&Classification::general(Internal), &compartments));
        assert_eq!(roles.check_role("unknown"), Err(RoleError::UnknownRole("unknown".to_string())));
        let typo = Roles::new([("x".to_string(), vec![Clearance { level: Internal, compartment: Some(compartment("hrr")) }])]).unwrap();
        assert!(matches!(typo.check(&compartments), Err(RoleError::UnknownCompartment { .. })));
        let written: Roles = serde_json::from_value(serde_json::json!([
            {"name": "hr", "clearances": [{"level": "confidential", "compartment": "hr"}, {"level": "internal"}]}
        ])).unwrap();
        assert!(written.clearances_of(&["hr".to_string()]).admits(&Classification::general(Internal), &compartments));
    }

    #[test]
    fn the_read_relation_is_closed_transitively() {
        let compartments = organisation();
        assert!(compartments.may_read(&compartment("hr_management"), &compartment("finance")), "through hr");
        assert!(compartments.may_read(&compartment("hr"), &compartment("hr")), "itself");
        assert!(!compartments.may_read(&compartment("finance"), &compartment("hr")), "not the other way round");
        assert!(!compartments.may_read(&compartment("sales"), &compartment("hr")));
    }

    #[test]
    fn compartments_are_refused_if_they_read_what_is_not_configured() {
        let refused = Compartments::new([(compartment("hr"), vec![compartment("payroll")])]);
        assert_eq!(refused, Err(ClassificationError::UnknownCompartment(compartment("payroll"))));
        let twice = Compartments::new([(compartment("hr"), vec![]), (compartment("hr"), vec![])]);
        assert_eq!(twice, Err(ClassificationError::DuplicateCompartment(compartment("hr"))));
        assert_eq!(organisation().check(&classified(Internal, &["hrr"])), Err(ClassificationError::UnknownCompartment(compartment("hrr"))));
        assert_eq!(organisation().check(&classified(Internal, &["hr"])), Ok(()));
    }

    #[test]
    fn information_flows_only_to_a_level_at_least_its_own() {
        let none = Compartments::default();
        assert!(Classification::general(Internal).flows_to(&Classification::general(Confidential), &none));
        assert!(Classification::general(Internal).flows_to(&Classification::general(Internal), &none));
        assert!(!Classification::general(Confidential).flows_to(&Classification::general(Internal), &none));
        assert!(Classification::general(Public).flows_to(&classified(Internal, &["hr"]), &none), "what is general flows anywhere");
    }

    #[test]
    fn information_with_an_audience_never_flows_into_a_general_destination() {
        let compartments = organisation();
        assert!(!classified(Internal, &["hr"]).flows_to(&Classification::general(Internal), &compartments));
        assert!(!classified(Internal, &["hr"]).flows_to(&Classification::general(StrictlyConfidential), &compartments));
    }

    #[test]
    fn every_audience_of_the_destination_must_be_covered() {
        let compartments = organisation();
        let hr = classified(Internal, &["hr"]);
        assert!(hr.flows_to(&classified(Internal, &["hr"]), &compartments));
        assert!(hr.flows_to(&classified(Internal, &["hr_management"]), &compartments), "HR management reads HR");
        assert!(classified(Internal, &["finance"]).flows_to(&classified(Internal, &["hr_management"]), &compartments), "transitively");
        assert!(!hr.flows_to(&classified(Internal, &["finance"]), &compartments), "finance does not read HR");
        assert!(!hr.flows_to(&classified(Internal, &["hr", "sales"]), &compartments), "sales would see it too");

        let released = classified(Internal, &["hr", "sales"]);
        assert!(released.flows_to(&classified(Internal, &["hr", "sales"]), &compartments), "released to both");
        assert!(released.flows_to(&classified(Internal, &["sales"]), &compartments), "any of its audiences will do");
    }
}
