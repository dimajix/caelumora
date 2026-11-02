//! How sensitive a piece of information is and for whom it is meant (issue 133): a level of a fixed
//! scale (`Confidentiality`) and an audience of compartments (`Compartment`), as the administrator
//! configured them and which reads which (`Compartments`), and the one flow rule over them
//! (`Classification::flows_to`).

use crate::query::FieldValue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::iter;

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

impl Confidentiality {
    /// Every level, from the lowest to the highest.
    pub const LEVELS: [Confidentiality; 4] = [
        Confidentiality::Public,
        Confidentiality::Internal,
        Confidentiality::Confidential,
        Confidentiality::StrictlyConfidential,
    ];
}

/// A level as a query compares it: as serde writes it, `"internal"`.
impl From<Confidentiality> for FieldValue {
    fn from(level: Confidentiality) -> Self {
        FieldValue::Text(level.to_string())
    }
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

/// A compartment as a query compares it: its name.
impl From<&Compartment> for FieldValue {
    fn from(compartment: &Compartment) -> Self {
        FieldValue::Text(compartment.0.clone())
    }
}

impl fmt::Display for Compartment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClassificationError {
    #[error("A public classification takes no compartment")]
    PublicWithCompartments,
    #[error("Compartment '{0}' is not configured")]
    UnknownCompartment(Compartment),
    #[error("A compartment needs a name")]
    UnnamedCompartment,
    #[error("Compartment '{0}' is configured twice")]
    DuplicateCompartment(Compartment),
    #[error("Compartments are given with a level")]
    CompartmentsWithoutLevel,
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

    /// The lowest classification: anyone may read it.
    pub const PUBLIC: Classification = Classification::general(Confidentiality::Public);

    /// `level` with the audience `compartments`; refused for `Public` with
    /// compartments. Whether the compartments exist is the configuration's
    /// question (`Compartments::check`).
    pub fn new(
        level: Confidentiality,
        compartments: impl IntoIterator<Item = Compartment>,
    ) -> Result<Self, ClassificationError> {
        let compartments: BTreeSet<Compartment> = compartments.into_iter().collect();
        if level == Confidentiality::Public && !compartments.is_empty() {
            return Err(ClassificationError::PublicWithCompartments);
        }
        Ok(Self { level, compartments })
    }

    /// `level` with the audience `compartments`, or `None` - whatever it is given to decides
    /// then - if no level is given, as for a file uploaded for a message (issue 282).
    ///
    /// # Errors
    /// `CompartmentsWithoutLevel` for compartments without a level, `PublicWithCompartments`.
    pub fn given(
        level: Option<Confidentiality>,
        compartments: impl IntoIterator<Item = Compartment>,
    ) -> Result<Option<Self>, ClassificationError> {
        let mut compartments = compartments.into_iter().peekable();
        match level {
            Some(level) => Self::new(level, compartments).map(Some),
            None if compartments.peek().is_none() => Ok(None),
            None => Err(ClassificationError::CompartmentsWithoutLevel),
        }
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
            && destination.compartments.iter().all(|reader| {
                self.compartments.iter().any(|source| compartments.may_read(reader, source))
            })
    }
}

impl Classification {
    /// This classification changed as little as it must to flow into
    /// `destination`, whatever compartments read which: its level at most
    /// the destination's, and the destination's compartments added to its
    /// audience - none if it is general already, or the destination is
    /// general, or it becomes public. What flows already stays as it is, up
    /// to the audiences added, which may read it anyway. What a channel's messages become when the
    /// channel is released to `destination` (issue 133).
    pub fn fitted_into(&self, destination: &Classification) -> Classification {
        let level = self.level.min(destination.level);
        let general = self.compartments.is_empty()
            || destination.compartments.is_empty()
            || level == Confidentiality::Public;
        let compartments = if general {
            BTreeSet::new()
        } else {
            self.compartments.union(&destination.compartments).cloned().collect()
        };
        Classification { level, compartments }
    }

    /// The classification of what holds information classified `self` and
    /// information classified `other` (issue 168): the higher level, and
    /// the audience both have in common - none if both are general, the
    /// other's if one is. Both flow into it, whatever compartments read
    /// which. Without a compartment that reads another it is the lowest
    /// such classification; with one, a lower one may exist that only the
    /// read relation shows, which is the rules' and not known here.
    /// `None` if the two have no compartment in common.
    pub fn join(&self, other: &Classification) -> Option<Classification> {
        let level = self.level.max(other.level);
        let compartments = match (self.compartments.is_empty(), other.compartments.is_empty()) {
            (true, _) => other.compartments.clone(),
            (false, true) => self.compartments.clone(),
            (false, false) => {
                let common: BTreeSet<Compartment> =
                    self.compartments.intersection(&other.compartments).cloned().collect();
                if common.is_empty() {
                    return None;
                }
                common
            }
        };
        Some(Classification { level, compartments })
    }

    /// The join of all `classifications` (`join`): `PUBLIC` for none,
    /// `None` if they have no compartment in common.
    pub fn join_all<'a>(
        classifications: impl IntoIterator<Item = &'a Classification>,
    ) -> Option<Classification> {
        classifications.into_iter().try_fold(Classification::PUBLIC, |joined, c| joined.join(c))
    }

    /// `self` joined with `other` (`join`) where the join is known to flow
    /// into `bound` (`surely_flows_to`); else `bound` - no compartment in
    /// common, or only by which compartments read which. What a piece of
    /// work writes as once it has seen `other` (issues 168, 212): it has
    /// seen nothing that does not flow into its bound, so the bound holds
    /// all of it.
    pub fn join_within(&self, other: &Classification, bound: &Classification) -> Classification {
        self.join(other)
            .filter(|joined| joined.surely_flows_to(bound))
            .unwrap_or_else(|| bound.clone())
    }

    /// Whether what is classified `self` flows into `destination` whatever
    /// compartments read which (issue 168): `flows_to` as if none read
    /// another. What does flows under every read relation.
    pub fn surely_flows_to(&self, destination: &Classification) -> bool {
        self.flows_to(destination, &Compartments::default())
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
    pub fn new(
        definitions: impl IntoIterator<Item = (Compartment, Vec<Compartment>)>,
    ) -> Result<Self, ClassificationError> {
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
                let further: BTreeSet<Compartment> =
                    readable.iter().flat_map(|c| snapshot[c].iter().cloned()).collect();
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

    /// The compartments whose audience `reader`'s may read (`may_read`):
    /// itself, and those it reads.
    pub fn readable_by<'a>(
        &'a self,
        reader: &'a Compartment,
    ) -> impl Iterator<Item = &'a Compartment> {
        iter::once(reader).chain(
            self.reads.get(reader).into_iter().flatten().filter(move |source| *source != reader),
        )
    }

    /// Nothing if every compartment of `classification` is configured.
    pub fn check(&self, classification: &Classification) -> Result<(), ClassificationError> {
        match classification.compartments.iter().find(|c| !self.reads.contains_key(*c)) {
            Some(unknown) => Err(ClassificationError::UnknownCompartment(unknown.clone())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::testing::{classified, compartment, organisation};
    use crate::query::meets_exactly_its_equals;
    use Confidentiality::*;

    #[test]
    fn a_level_and_a_compartment_meet_exactly_their_serialized_forms() {
        meets_exactly_its_equals(&Confidentiality::LEVELS);
        let samples = [Compartment::new("hr"), Compartment::new("finance")];
        for compartment in &samples {
            for other in &samples {
                assert_eq!(
                    FieldValue::from(compartment).matches(&serde_json::to_value(other).unwrap()),
                    compartment == other
                );
            }
        }
    }

    #[test]
    fn the_levels_are_ordered_by_sensitivity() {
        assert!(
            Public < Internal && Internal < Confidential && Confidential < StrictlyConfidential
        );
        assert_eq!(serde_json::to_value(StrictlyConfidential).unwrap(), "strictly_confidential");
        assert_eq!(Classification::default(), Classification::general(Internal));
    }

    #[test]
    fn public_takes_no_compartment() {
        assert_eq!(
            Classification::new(Public, [compartment("hr")]),
            Err(ClassificationError::PublicWithCompartments)
        );
        let written = serde_json::json!({"level": "public", "compartments": ["hr"]});
        assert!(
            serde_json::from_value::<Classification>(written).is_err(),
            "not even when it is read"
        );
    }

    #[test]
    fn a_classification_given_without_a_level_is_left_to_its_destination() {
        assert_eq!(Classification::given(None, []), Ok(None));
        assert_eq!(
            Classification::given(Some(Confidential), [compartment("hr")]),
            Ok(Some(classified(Confidential, &["hr"])))
        );
        assert_eq!(
            Classification::given(None, [compartment("hr")]),
            Err(ClassificationError::CompartmentsWithoutLevel)
        );
        assert_eq!(
            Classification::given(Some(Public), [compartment("hr")]),
            Err(ClassificationError::PublicWithCompartments)
        );
    }

    #[test]
    fn a_classification_is_written_without_an_empty_audience() {
        assert_eq!(
            serde_json::to_value(Classification::general(Internal)).unwrap(),
            serde_json::json!({"level": "internal"})
        );
        let hr = classified(Confidential, &["hr"]);
        assert_eq!(
            serde_json::to_value(&hr).unwrap(),
            serde_json::json!({"level": "confidential", "compartments": ["hr"]})
        );
        assert_eq!(
            serde_json::from_value::<Classification>(
                serde_json::json!({"level": "confidential", "compartments": ["hr"]})
            )
            .unwrap(),
            hr
        );
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
        assert_eq!(
            fitted(hr_confidential.clone(), classified(Internal, &["hr"])),
            classified(Internal, &["hr"]),
            "the level lowered"
        );
        assert_eq!(
            fitted(classified(Confidential, &["hr", "sales"]), classified(Internal, &["hr"])),
            classified(Internal, &["hr", "sales"]),
            "keeps its wider audience"
        );
        assert_eq!(
            fitted(classified(Internal, &["hr"]), classified(Internal, &["hr", "sales"])),
            classified(Internal, &["hr", "sales"]),
            "released to the new audience"
        );
        assert_eq!(
            fitted(hr_confidential.clone(), Classification::general(Internal)),
            Classification::general(Internal),
            "released to everyone"
        );
        assert_eq!(
            fitted(hr_confidential.clone(), Classification::general(Public)),
            Classification::general(Public)
        );
        assert_eq!(
            fitted(classified(Internal, &["hr"]), classified(StrictlyConfidential, &["hr"])),
            classified(Internal, &["hr"]),
            "raising changes nothing"
        );
        assert_eq!(
            fitted(Classification::general(Internal), classified(Internal, &["hr"])),
            Classification::general(Internal),
            "what is general stays general"
        );
    }

    #[test]
    fn what_holds_two_classifications_is_their_join_into_which_both_flow() {
        let compartments = organisation();
        let joined = |a: Classification, b: Classification| {
            let joined = a.join(&b);
            assert_eq!(joined, b.join(&a), "in either order");
            if let Some(joined) = &joined {
                assert!(
                    a.flows_to(joined, &compartments) && b.flows_to(joined, &compartments),
                    "{a} and {b} flow into {joined}"
                );
            }
            joined
        };
        assert_eq!(
            joined(Classification::PUBLIC, classified(Internal, &["hr"])),
            Some(classified(Internal, &["hr"])),
            "public adds nothing"
        );
        assert_eq!(
            joined(Classification::general(Confidential), classified(Internal, &["hr"])),
            Some(classified(Confidential, &["hr"])),
            "the higher level, the audience"
        );
        assert_eq!(
            joined(classified(Internal, &["hr", "sales"]), classified(Internal, &["hr"])),
            Some(classified(Internal, &["hr"])),
            "the audience in common"
        );
        assert_eq!(
            joined(classified(Internal, &["hr"]), classified(Internal, &["sales"])),
            None,
            "no audience in common"
        );
        assert_eq!(Classification::join_all([]), Some(Classification::PUBLIC));
        assert_eq!(
            Classification::join_all(&[
                Classification::general(Internal),
                classified(Confidential, &["hr", "sales"]),
                classified(Internal, &["sales"])
            ]),
            Some(classified(Confidential, &["sales"]))
        );
    }

    #[test]
    fn a_join_within_a_bound_is_the_join_where_it_surely_flows_and_else_the_bound() {
        let bound = classified(Confidential, &["hr"]);
        assert_eq!(
            Classification::PUBLIC.join_within(&Classification::general(Internal), &bound),
            Classification::general(Internal),
            "within it: the join"
        );
        assert_eq!(
            Classification::general(Internal)
                .join_within(&classified(Internal, &["hr", "sales"]), &bound),
            classified(Internal, &["hr", "sales"]),
            "a wider audience than the bound's"
        );
        assert_eq!(
            classified(Internal, &["hr"]).join_within(&classified(Internal, &["sales"]), &bound),
            bound,
            "no audience in common: the bound"
        );
        assert_eq!(
            classified(Internal, &["hr", "sales"])
                .join_within(&classified(Internal, &["sales"]), &bound),
            bound,
            "not known to flow into the bound: the bound"
        );
        let hr_management = classified(Internal, &["hr_management"]);
        assert_eq!(
            Classification::PUBLIC.join_within(&classified(Internal, &["hr"]), &hr_management),
            hr_management,
            "only by the read relation: the bound"
        );
    }

    #[test]
    fn what_surely_flows_flows_whatever_compartments_read() {
        let compartments = organisation();
        assert!(classified(Internal, &["hr", "sales"])
            .surely_flows_to(&classified(Confidential, &["hr"])));
        assert!(Classification::general(Internal).surely_flows_to(&classified(Internal, &["hr"])));
        assert!(
            !classified(Internal, &["hr"]).surely_flows_to(&Classification::general(Internal)),
            "an audience never into what is general"
        );
        let hr_management = classified(Internal, &["hr_management"]);
        assert!(classified(Internal, &["hr"]).flows_to(&hr_management, &compartments));
        assert!(
            !classified(Internal, &["hr"]).surely_flows_to(&hr_management),
            "only by the read relation"
        );
    }
    #[test]
    fn the_read_relation_is_closed_transitively() {
        let compartments = organisation();
        assert!(
            compartments.may_read(&compartment("hr_management"), &compartment("finance")),
            "through hr"
        );
        assert!(compartments.may_read(&compartment("hr"), &compartment("hr")), "itself");
        assert!(
            !compartments.may_read(&compartment("finance"), &compartment("hr")),
            "not the other way round"
        );
        assert!(!compartments.may_read(&compartment("sales"), &compartment("hr")));
    }

    #[test]
    fn compartments_are_refused_if_they_read_what_is_not_configured() {
        let refused = Compartments::new([(compartment("hr"), vec![compartment("payroll")])]);
        assert_eq!(refused, Err(ClassificationError::UnknownCompartment(compartment("payroll"))));
        let twice = Compartments::new([(compartment("hr"), vec![]), (compartment("hr"), vec![])]);
        assert_eq!(twice, Err(ClassificationError::DuplicateCompartment(compartment("hr"))));
        assert_eq!(
            organisation().check(&classified(Internal, &["hrr"])),
            Err(ClassificationError::UnknownCompartment(compartment("hrr")))
        );
        assert_eq!(organisation().check(&classified(Internal, &["hr"])), Ok(()));
    }

    #[test]
    fn information_flows_only_to_a_level_at_least_its_own() {
        let none = Compartments::default();
        assert!(Classification::general(Internal)
            .flows_to(&Classification::general(Confidential), &none));
        assert!(
            Classification::general(Internal).flows_to(&Classification::general(Internal), &none)
        );
        assert!(!Classification::general(Confidential)
            .flows_to(&Classification::general(Internal), &none));
        assert!(
            Classification::general(Public).flows_to(&classified(Internal, &["hr"]), &none),
            "what is general flows anywhere"
        );
    }

    #[test]
    fn information_with_an_audience_never_flows_into_a_general_destination() {
        let compartments = organisation();
        assert!(!classified(Internal, &["hr"])
            .flows_to(&Classification::general(Internal), &compartments));
        assert!(!classified(Internal, &["hr"])
            .flows_to(&Classification::general(StrictlyConfidential), &compartments));
    }

    #[test]
    fn every_audience_of_the_destination_must_be_covered() {
        let compartments = organisation();
        let hr = classified(Internal, &["hr"]);
        assert!(hr.flows_to(&classified(Internal, &["hr"]), &compartments));
        assert!(
            hr.flows_to(&classified(Internal, &["hr_management"]), &compartments),
            "HR management reads HR"
        );
        assert!(
            classified(Internal, &["finance"])
                .flows_to(&classified(Internal, &["hr_management"]), &compartments),
            "transitively"
        );
        assert!(
            !hr.flows_to(&classified(Internal, &["finance"]), &compartments),
            "finance does not read HR"
        );
        assert!(
            !hr.flows_to(&classified(Internal, &["hr", "sales"]), &compartments),
            "sales would see it too"
        );

        let released = classified(Internal, &["hr", "sales"]);
        assert!(
            released.flows_to(&classified(Internal, &["hr", "sales"]), &compartments),
            "released to both"
        );
        assert!(
            released.flows_to(&classified(Internal, &["sales"]), &compartments),
            "any of its audiences will do"
        );
    }
}
