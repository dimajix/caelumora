//! Classification (issue 133): how sensitive a piece of information is, and
//! for whom it is meant.
//!
//! A `Classification` is a level from a fixed scale and an audience of
//! compartments - areas such as a topic, a group or a customer, which the
//! administrator configures (`Compartments`). What may flow where is decided
//! by the one flow rule, `Classification::flows_to`, from the
//! classifications involved alone - not by who takes part in a channel at
//! the moment, so that a decision stays the same when someone joins or
//! leaves. The decisions are a `ConfidentialityPolicy`'s, apart from who may
//! do what (`AccessPolicy`) and asked beside it (issue 304);
//! `ConfidentialityRules` is the one Caelumora applies.
//!
//! Whether anyone but an actor's owner sees something at all is a separate
//! axis, `Visibility`: ownership, not sensitivity.

use crate::actor::ActorId;
use crate::message::Message;
use crate::privacy::SessionPrivacy;
use crate::role::{RoleId, Roles};
use crate::security::{ActionType, SecurityContext, AccessPolicy};
use crate::session::ChannelId;
use crate::store::query::FieldValue;
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
    pub const LEVELS: [Confidentiality; 4] = [Confidentiality::Public, Confidentiality::Internal, Confidentiality::Confidential, Confidentiality::StrictlyConfidential];
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
    /// to the audiences added, which may read it anyway. What a channel's messages become when the
    /// channel is released to `destination` (issue 133).
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
                let common: BTreeSet<Compartment> = self.compartments.intersection(&other.compartments).cloned().collect();
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
    pub fn join_all<'a>(classifications: impl IntoIterator<Item = &'a Classification>) -> Option<Classification> {
        classifications.into_iter().try_fold(Classification::PUBLIC, |joined, c| joined.join(c))
    }

    /// `self` joined with `other` (`join`) where the join is known to flow
    /// into `bound` (`surely_flows_to`); else `bound` - no compartment in
    /// common, or only by which compartments read which. What a piece of
    /// work writes as once it has seen `other` (issues 168, 212): it has
    /// seen nothing that does not flow into its bound, so the bound holds
    /// all of it.
    pub fn join_within(&self, other: &Classification, bound: &Classification) -> Classification {
        self.join(other).filter(|joined| joined.surely_flows_to(bound)).unwrap_or_else(|| bound.clone())
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

/// Which of the classifications in a message's context set its label
/// (issue 170). The label of a message is the join of everything its turn
/// had seen, at most the channel's classification (issue 168); what set it
/// is derived from that, nothing about it is stored:
///
/// - what was not classified counted as the channel, and so set the label
///   to the channel's;
/// - if the classified parts had no audience in common, the label is the
///   channel's, set by the parts with an audience;
/// - otherwise the parts at the label's level set it.
#[derive(Debug)]
pub struct LabelCauses {
    label: Classification,
    disjoint: bool,
}

impl LabelCauses {
    /// The causes of `label`, the classification of a message whose
    /// context's parts are classified `classifications` - those known.
    pub fn new<'a>(label: Classification, classifications: impl IntoIterator<Item = &'a Classification>) -> Self {
        let disjoint = Classification::join_all(classifications).is_none();
        Self { label, disjoint }
    }

    /// Whether a part classified `classification` - `None`: not known -
    /// set the label.
    pub fn set_by(&self, classification: Option<&Classification>) -> bool {
        match classification {
            None => true,
            Some(c) if self.disjoint => !c.compartments().is_empty(),
            Some(c) => c.level() == self.label.level(),
        }
    }

    /// Whether `message` set the label, by its own classification or that
    /// of a tool result in it.
    pub fn set_by_message(&self, message: &Message) -> bool {
        self.set_by(Some(&message.classification)) || message.tool_results.iter().any(|r| self.set_by(Some(&r.classification)))
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

    /// The compartments whose audience `reader`'s may read (`may_read`):
    /// itself, and those it reads.
    pub fn readable_by<'a>(&'a self, reader: &'a Compartment) -> impl Iterator<Item = &'a Compartment> {
        iter::once(reader).chain(self.reads.get(reader).into_iter().flatten().filter(move |source| *source != reader))
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
    /// These clearances.
    pub(crate) fn new(clearances: Vec<Clearance>) -> Self {
        Self(clearances)
    }

    /// Whether what is classified `classification` may be seen - by one of
    /// the clearances, or because it is public.
    pub fn admits(&self, classification: &Classification, compartments: &Compartments) -> bool {
        classification.level == Confidentiality::Public || self.0.iter().any(|c| c.sees(classification, compartments))
    }
}

/// One side of a copy (`ConfidentialityQuestions::may_copy_information`): a channel, the
/// classification of what is read there or of the channel that reads, and
/// the session it belongs to as privacy sees it (issue 134). Whether two
/// places are one channel, or in one session, is told by their ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub channel: ChannelId,
    pub classification: Classification,
    pub session: SessionPrivacy,
}

/// How far a reader reads what lies at a place (issues 133, 421): the whole,
/// what was released to it - each entry classified so that it flows into a
/// classification, the reader's -, or nothing. A reader's relation to a
/// channel: the bound is the reader's, the channel decides whether it holds
/// at all (its session's consent) or is not needed (it flows whole). Asked
/// of a `ConfidentialityPolicy` as what of a channel may be copied into a
/// place, once; data rather than a decision for each entry, so that a store
/// reads only what may be read (`HistoryField`). How far, not who reads -
/// that is `history::Reader`, whom a store gives which entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Whole,
    Released(Classification),
    Nothing,
}

impl Reading {
    /// Whether the reader reads what is classified `classification`, with
    /// the configured `compartments`.
    pub fn admits(&self, classification: &Classification, compartments: &Compartments) -> bool {
        match self {
            Reading::Whole => true,
            Reading::Released(into) => classification.flows_to(into, compartments),
            Reading::Nothing => false,
        }
    }
}

/// The flow decisions (issues 133, 304): whether information may go where
/// an act takes it, and whether someone is cleared for it - apart from who
/// may do what, which is the `AccessPolicy`'s, and asked beside it, after
/// the act's own permission. Each is decided from the context, the
/// classifications and the places it is given alone; nothing is looked up
/// while it is made. `ConfidentialityRules` is the policy Caelumora applies;
/// one defined outside the binary (issue 24) states its own.
pub trait ConfidentialityPolicy: fmt::Debug + Send + Sync {
    /// Whether what is classified `classification` may be put, in
    /// `context`, into a destination classified `into` - a channel's
    /// history, an agent's context, a storage that keeps it.
    fn may_publish(&self, context: &SecurityContext, classification: &Classification, into: &Classification) -> bool;

    /// What of the channel `channel`, in a session with the privacy
    /// `session`, may be copied, in `context`, to `into`, where an agent
    /// reads it (issues 17, 134, 264): not a decision for each
    /// classification, but which classifications - data, so that a store
    /// reads only what may be copied (issue 421).
    fn copyable(&self, context: &SecurityContext, channel: ChannelId, session: &SessionPrivacy, into: &Place) -> Reading;

    /// Whether `participant`, by `roles`, is cleared for what is classified
    /// `classification` - asked in `context`, which may be another's: whoever
    /// admits the participant.
    fn cleared(&self, context: &SecurityContext, participant: ActorId, roles: &[RoleId], classification: &Classification) -> bool;

    /// The configured compartments: what a classification may name. Giving
    /// one that names others is invalid, not refused (`require_classifiable`).
    fn compartments(&self) -> &Compartments;

    /// The configured roles, by which ids and names are translated; `None`
    /// where clearances are not checked.
    fn roles(&self) -> Option<&Roles>;
}

/// The questions asked of a `ConfidentialityPolicy`, each with its name and
/// in one place, as `AccessQuestions` and `Enforcement` are of an
/// `AccessPolicy`: the `may_*` to decide what to offer, the `require_*`
/// for an act, answering a refusal with the act's own error and logging it
/// alike (issue 304).
pub trait ConfidentialityQuestions {
    /// What of the channel `channel`, in a session with the privacy
    /// `session`, may be copied to `into` in `context`: of a channel into
    /// itself the whole, without asking.
    fn copyable_information(&self, context: &SecurityContext, channel: ChannelId, session: &SessionPrivacy, into: &Place) -> Reading;

    /// Whether what is at `from` may be copied to `into` in `context`
    /// (`copyable_information`).
    fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool;

    /// Whether `context` is cleared for `classification` by the roles of its
    /// owner, who answers for the caller - and, for a caller limited to
    /// roles, by those as well (issues 179, 341): what decides which
    /// channels, and what else classified, a viewer is shown.
    /// The system is cleared for everything, as it may do everything
    /// (issue 312).
    fn cleared_for(&self, context: &SecurityContext, classification: &Classification) -> bool;

    /// Nothing if what is classified `classification` may be put into a
    /// destination classified `into`; `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if it may not.
    fn require_publish<E>(&self, context: &SecurityContext, classification: &Classification, into: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E>;

    /// Nothing if `participant`, by `roles`, is cleared for `classification`;
    /// `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if they are not.
    fn require_cleared<E>(&self, context: &SecurityContext, participant: ActorId, roles: &[RoleId], classification: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E>;

    /// Nothing if `context` is cleared for `classification` (`cleared_for`);
    /// `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if it is not.
    fn require_cleared_for<E>(&self, context: &SecurityContext, classification: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E>;

    /// Nothing if `classification` names only configured compartments;
    /// `invalid` otherwise - a classification that cannot be given at all,
    /// which no permission changes.
    ///
    /// # Errors
    /// `invalid()` if it is not.
    fn require_classifiable<E>(&self, classification: &Classification, invalid: impl FnOnce() -> E) -> Result<(), E>;
}

impl<T: ConfidentialityPolicy + ?Sized> ConfidentialityQuestions for T {
    fn copyable_information(&self, context: &SecurityContext, channel: ChannelId, session: &SessionPrivacy, into: &Place) -> Reading {
        match channel == into.channel {
            true => Reading::Whole,
            false => self.copyable(context, channel, session, into),
        }
    }

    fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool {
        self.copyable_information(context, from.channel, &from.session, into).admits(&from.classification, self.compartments())
    }

    fn cleared_for(&self, context: &SecurityContext, classification: &Classification) -> bool {
        let participant = context.owner_id;
        if participant == ActorId::System {
            return true;
        }
        self.cleared(context, participant, &context.roles, classification)
            && context.limiting_roles.as_ref().is_none_or(|limit| self.cleared(context, participant, limit, classification))
    }

    fn require_publish<E>(&self, context: &SecurityContext, classification: &Classification, into: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E> {
        refused(self.may_publish(context, classification, into), context, || format!("publishing {classification} into {into}"), refusal)
    }

    fn require_cleared<E>(&self, context: &SecurityContext, participant: ActorId, roles: &[RoleId], classification: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E> {
        refused(self.cleared(context, participant, roles, classification), context, || format!("{participant} not cleared for {classification}"), refusal)
    }

    fn require_cleared_for<E>(&self, context: &SecurityContext, classification: &Classification, refusal: impl FnOnce() -> E) -> Result<(), E> {
        refused(self.cleared_for(context, classification), context, || format!("not cleared for {classification}"), refusal)
    }

    fn require_classifiable<E>(&self, classification: &Classification, invalid: impl FnOnce() -> E) -> Result<(), E> {
        match self.compartments().check(classification) {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::debug!("Refused to classify as {classification}: {e}");
                Err(invalid())
            }
        }
    }
}

/// Whether `context` reaches what `read` is asked of and is classified as
/// `classifications` says (concept 4 of the permission checks, issue 322):
/// it may read it (`access_policy`), and, where it is classified, is cleared
/// for one of them (`confidentiality`) - one, the entity's; two as it is
/// reclassified, the one it had and the one it has. The question the store
/// asks of what it lists and the event bus of whom an event reaches, so
/// that both reach alike. A question, for filtering: an operation reads
/// through the store, which enforces it.
pub async fn may_reach(access_policy: &dyn AccessPolicy, confidentiality: &dyn ConfidentialityPolicy, context: &SecurityContext, read: ActionType<'_>, classifications: &[Classification]) -> bool {
    access_policy.check(context, read).await
        && (classifications.is_empty() || classifications.iter().any(|classification| confidentiality.cleared_for(context, classification)))
}

/// `Ok` if `allowed`, else `refusal()`, logged at `debug!` by what
/// `describe` names - never content.
fn refused<E>(allowed: bool, context: &SecurityContext, describe: impl FnOnce() -> String, refusal: impl FnOnce() -> E) -> Result<(), E> {
    if allowed {
        return Ok(());
    }
    tracing::debug!("Refused {} for {} as {}", describe(), context.principal_id, context.caller_id);
    Err(refusal())
}

/// The confidentiality rules as Caelumora applies them (issue 133): the
/// flow rule with the configured compartments, the session's consent to
/// being read from beyond it (issue 134), and the clearances of the
/// configured roles - the `ConfidentialityPolicy` of every configuration
/// (issue 304).
#[derive(Debug, Clone, Default)]
pub struct ConfidentialityRules {
    compartments: Compartments,
    /// `None`: clearances are not checked.
    roles: Option<Roles>,
}

impl ConfidentialityRules {
    /// The rules with the configured `compartments`, without checking
    /// clearances.
    pub fn new(compartments: Compartments) -> Self {
        Self { compartments, roles: None }
    }

    /// These rules, checking clearances by the configured `roles`.
    pub fn with_roles(self, roles: Roles) -> Self {
        Self { roles: Some(roles), ..self }
    }
}

impl ConfidentialityPolicy for ConfidentialityRules {
    /// If it names only configured compartments and flows into `into`.
    fn may_publish(&self, _context: &SecurityContext, classification: &Classification, into: &Classification) -> bool {
        self.compartments.check(classification).is_ok() && classification.flows_to(into, &self.compartments)
    }

    /// A channel into itself the whole; within a session what flows into
    /// `into`; beyond it only where the session of the channel permits it
    /// as well (issue 134), and nothing otherwise - whatever the
    /// classification, `public` included.
    fn copyable(&self, _context: &SecurityContext, channel: ChannelId, session: &SessionPrivacy, into: &Place) -> Reading {
        if channel == into.channel {
            return Reading::Whole;
        }
        match session.id == into.session.id || session.permits(&into.session) {
            true => Reading::Released(into.classification.clone()),
            false => Reading::Nothing,
        }
    }

    /// By the clearances the configured roles grant. Without roles
    /// configured, everybody is.
    fn cleared(&self, _context: &SecurityContext, _participant: ActorId, roles: &[RoleId], classification: &Classification) -> bool {
        match &self.roles {
            Some(configured) => configured.clearances_of(roles).admits(classification, &self.compartments),
            None => true,
        }
    }

    fn compartments(&self) -> &Compartments {
        &self.compartments
    }

    fn roles(&self) -> Option<&Roles> {
        self.roles.as_ref()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::query::meets_exactly_its_equals;
    use crate::agent::AgentId;
    use crate::user::UserId;
    use crate::tool::{ToolCall, ToolCallResult};
    use Confidentiality::*;
    use crate::session::SessionId;

    #[test]
    fn a_level_and_a_compartment_meet_exactly_their_serialized_forms() {
        meets_exactly_its_equals(&Confidentiality::LEVELS);
        let samples = [Compartment::new("hr"), Compartment::new("finance")];
        for compartment in &samples {
            for other in &samples {
                assert_eq!(FieldValue::from(compartment).matches(&serde_json::to_value(other).unwrap()), compartment == other);
            }
        }
    }

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
    fn what_holds_two_classifications_is_their_join_into_which_both_flow() {
        let compartments = organisation();
        let joined = |a: Classification, b: Classification| {
            let joined = a.join(&b);
            assert_eq!(joined, b.join(&a), "in either order");
            if let Some(joined) = &joined {
                assert!(a.flows_to(joined, &compartments) && b.flows_to(joined, &compartments), "{a} and {b} flow into {joined}");
            }
            joined
        };
        assert_eq!(joined(Classification::PUBLIC, classified(Internal, &["hr"])), Some(classified(Internal, &["hr"])), "public adds nothing");
        assert_eq!(joined(Classification::general(Confidential), classified(Internal, &["hr"])), Some(classified(Confidential, &["hr"])), "the higher level, the audience");
        assert_eq!(joined(classified(Internal, &["hr", "sales"]), classified(Internal, &["hr"])), Some(classified(Internal, &["hr"])), "the audience in common");
        assert_eq!(joined(classified(Internal, &["hr"]), classified(Internal, &["sales"])), None, "no audience in common");
        assert_eq!(Classification::join_all([]), Some(Classification::PUBLIC));
        assert_eq!(Classification::join_all(&[Classification::general(Internal), classified(Confidential, &["hr", "sales"]), classified(Internal, &["sales"])]), Some(classified(Confidential, &["sales"])));
    }

    #[test]
    fn a_join_within_a_bound_is_the_join_where_it_surely_flows_and_else_the_bound() {
        let bound = classified(Confidential, &["hr"]);
        assert_eq!(Classification::PUBLIC.join_within(&Classification::general(Internal), &bound), Classification::general(Internal), "within it: the join");
        assert_eq!(Classification::general(Internal).join_within(&classified(Internal, &["hr", "sales"]), &bound), classified(Internal, &["hr", "sales"]), "a wider audience than the bound's");
        assert_eq!(classified(Internal, &["hr"]).join_within(&classified(Internal, &["sales"]), &bound), bound, "no audience in common: the bound");
        assert_eq!(classified(Internal, &["hr", "sales"]).join_within(&classified(Internal, &["sales"]), &bound), bound, "not known to flow into the bound: the bound");
        let hr_management = classified(Internal, &["hr_management"]);
        assert_eq!(Classification::PUBLIC.join_within(&classified(Internal, &["hr"]), &hr_management), hr_management, "only by the read relation: the bound");
    }

    #[test]
    fn what_surely_flows_flows_whatever_compartments_read() {
        let compartments = organisation();
        assert!(classified(Internal, &["hr", "sales"]).surely_flows_to(&classified(Confidential, &["hr"])));
        assert!(Classification::general(Internal).surely_flows_to(&classified(Internal, &["hr"])));
        assert!(!classified(Internal, &["hr"]).surely_flows_to(&Classification::general(Internal)), "an audience never into what is general");
        let hr_management = classified(Internal, &["hr_management"]);
        assert!(classified(Internal, &["hr"]).flows_to(&hr_management, &compartments));
        assert!(!classified(Internal, &["hr"]).surely_flows_to(&hr_management), "only by the read relation");
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

    #[test]
    fn the_rules_give_only_configured_compartments_and_check_clearances_only_with_roles() {
        let rules = ConfidentialityRules::new(organisation());
        let anyone = SecurityContext::default();
        let classifiable = |c: &Classification| rules.require_classifiable(c, || ()).is_ok();
        assert!(classifiable(&classified(Internal, &["hr"])) && !classifiable(&classified(Internal, &["hrr"])));
        assert!(rules.may_publish(&anyone, &classified(Internal, &["hr"]), &classified(Confidential, &["hr_management"])));
        assert!(!rules.may_publish(&anyone, &classified(Internal, &["hrr"]), &classified(Internal, &["hrr"])), "not configured, though it would flow");
        assert!(!rules.may_publish(&anyone, &classified(Confidential, &["hr"]), &classified(Internal, &["hr"])), "it would not flow");

        let secret = classified(Confidential, &["hr"]);
        let nobody = ActorId::None;
        assert!(rules.cleared(&anyone, nobody, &[], &secret), "without roles, clearances are not checked");
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }])]).unwrap();
        let rules = rules.with_roles(roles);
        assert_eq!(rules.roles().and_then(|r| r.name_of(hr)), Some("hr"));
        assert!(rules.cleared(&anyone, nobody, &[hr], &secret));
        assert!(!rules.cleared(&anyone, nobody, &[], &secret) && rules.cleared(&anyone, nobody, &[], &Classification::general(Public)), "with roles, nobody else but for what is public");
    }

    /// Issue 304: a viewer is cleared by the roles of their context, and a
    /// caller limited to roles by those as well.
    #[test]
    fn a_context_is_cleared_by_its_roles_and_its_limit() {
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }])]).unwrap();
        let rules = ConfidentialityRules::new(organisation()).with_roles(roles);
        let secret = classified(Confidential, &["hr"]);
        let alice = SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default());
        assert!(!rules.cleared_for(&alice, &secret), "without roles");
        let hers = alice.clone().with_roles(vec![hr], None);
        assert!(rules.cleared_for(&hers, &secret));
        assert!(!rules.cleared_for(&alice.with_roles(vec![hr], Some(vec![])), &secret), "limited below it");
        assert!(rules.require_cleared_for(&hers, &secret, || ()).is_ok());
    }

    /// Issue 341: a caller is cleared by its owner, who answers for it -
    /// whoever it acts for lends no clearance, not even the system.
    #[test]
    fn a_caller_is_cleared_as_its_owner_whoever_it_acts_for() {
        let hr = RoleId::new();
        let roles = Roles::new([(hr, "hr".to_string(), vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }])]).unwrap();
        let rules = ConfidentialityRules::new(organisation()).with_roles(roles);
        let secret = classified(Confidential, &["hr"]);
        let owner = ActorId::User(UserId::new());
        let agent = ActorId::Agent(AgentId::new());
        let for_the_system = SecurityContext::system().next(agent, owner, ActorId::System);
        assert!(!rules.cleared_for(&for_the_system, &secret), "acting for the system, still bounded by its owner");
        let owned_by_the_system = SecurityContext::system().next(agent, ActorId::System, owner);
        assert!(rules.cleared_for(&owned_by_the_system, &secret), "the system's agent is cleared as the system");
    }

    /// A channel of the session `session_id`, a private one of nobody.
    fn place(session_id: SessionId, level: Confidentiality, compartments: &[&str]) -> Place {
        in_session(SessionPrivacy { id: session_id, ..SessionPrivacy::default() }, level, compartments)
    }

    fn in_session(session: SessionPrivacy, level: Confidentiality, compartments: &[&str]) -> Place {
        Place { channel: ChannelId::new(), classification: classified(level, compartments), session }
    }

    fn hr_reads_finance() -> ConfidentialityRules {
        ConfidentialityRules::new(Compartments::new([(compartment("hr"), vec![compartment("finance")]), (compartment("finance"), vec![])]).unwrap())
    }

    fn someone() -> SecurityContext {
        SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default())
    }

    #[test]
    fn reading_a_channel_depends_on_the_channels_not_on_the_reader() {
        let (rules, session) = (ConfidentialityRules::default(), SessionId::new());
        let (team, secret) = (place(session, Internal, &[]), place(session, Confidential, &[]));
        for reader in [someone(), someone()] {
            assert!(rules.may_copy_information(&reader, &team, &secret));
            assert!(!rules.may_copy_information(&reader, &secret, &team));
        }
    }

    #[test]
    fn reading_a_channel_follows_the_flow_rule_with_the_configured_compartments() {
        let (rules, session, alice) = (hr_reads_finance(), SessionId::new(), someone());
        let (hr, finance, general) = (place(session, Internal, &["hr"]), place(session, Internal, &["finance"]), place(session, Internal, &[]));
        assert!(rules.may_copy_information(&alice, &finance, &hr), "HR reads finance");
        assert!(!rules.may_copy_information(&alice, &hr, &finance), "not the other way round");
        assert!(!rules.may_copy_information(&alice, &hr, &general), "nor a general channel");
        assert!(rules.may_copy_information(&alice, &general, &hr), "what is general flows anywhere");
    }

    #[test]
    fn a_channel_reads_another_session_only_where_that_session_permits_it() {
        use crate::privacy::PrivacyConfig;
        let (alice, bob) = (UserId::new(), UserId::new());
        let hers = |policy, others: &[UserId]| SessionPrivacy {
            id: SessionId::new(),
            policy,
            owner: ActorId::User(alice),
            people: others.iter().copied().chain([alice]).collect(),
        };
        let rules = ConfidentialityRules::default();
        let reader = in_session(hers(PrivacyConfig::Private, &[]), Internal, &[]);
        let elsewhere = |policy, level| in_session(hers(policy, &[bob]), level, &[]);
        let agent = SecurityContext::new(ActorId::Agent(AgentId::new()), Default::default(), Default::default());
        assert!(!rules.may_copy_information(&agent, &elsewhere(PrivacyConfig::Private, Internal), &reader), "a private session is read by no other");
        assert!(!rules.may_copy_information(&agent, &elsewhere(PrivacyConfig::Private, Public), &reader), "not even what is public");
        assert!(rules.may_copy_information(&agent, &elsewhere(PrivacyConfig::SameOwner, Internal), &reader), "her other sessions, where she lets them");
        assert!(rules.may_copy_information(&agent, &elsewhere(PrivacyConfig::SharedAudience, Internal), &reader), "she took part there");
        assert!(!rules.may_copy_information(&agent, &elsewhere(PrivacyConfig::SameOwner, Confidential), &reader), "and still only what flows into the reader");
    }

    #[test]
    fn what_is_released_below_its_channel_is_read_where_the_channel_is_not() {
        let (rules, session, alice) = (hr_reads_finance(), SessionId::new(), someone());
        let (hr, finance) = (place(session, Confidential, &["hr"]), place(session, Confidential, &["finance"]));
        assert!(!rules.may_copy_information(&alice, &hr, &finance), "finance does not read HR");
        let released = Place { classification: classified(Confidential, &["hr", "finance"]), ..hr.clone() };
        assert!(rules.may_copy_information(&alice, &released, &finance), "but what HR released to it");
        let elsewhere = place(SessionId::new(), Confidential, &["finance"]);
        assert!(!rules.may_copy_information(&alice, &released, &elsewhere), "not from another session");
    }

    #[test]
    fn what_is_published_into_a_channel_has_to_flow_into_it() {
        let (rules, alice) = (hr_reads_finance(), someone());
        let hr = classified(Confidential, &["hr"]);
        assert!(rules.may_publish(&alice, &hr, &hr), "the channel's own");
        assert!(rules.may_publish(&alice, &classified(Internal, &["hr", "finance"]), &hr), "released to finance, at a lower level");
        assert!(!rules.may_publish(&alice, &classified(StrictlyConfidential, &["hr"]), &hr), "above the channel - never reclassified there either");
        assert!(rules.may_publish(&alice, &classified(Confidential, &["finance"]), &hr), "HR may read what is finance's");
        assert!(rules.may_publish(&alice, &Classification::general(Confidential), &hr), "what is general flows into any channel");
        assert!(!rules.may_publish(&alice, &classified(Confidential, &["hr", "payroll"]), &hr), "not configured");
    }

    #[test]
    fn a_channel_is_copied_into_itself_whatever_the_policy() {
        /// Refuses every flow.
        #[derive(Debug)]
        struct DenyAll(Compartments);
        impl ConfidentialityPolicy for DenyAll {
            fn may_publish(&self, _: &SecurityContext, _: &Classification, _: &Classification) -> bool { false }
            fn copyable(&self, _: &SecurityContext, _: ChannelId, _: &SessionPrivacy, _: &Place) -> Reading { Reading::Nothing }
            fn cleared(&self, _: &SecurityContext, _: ActorId, _: &[RoleId], _: &Classification) -> bool { false }
            fn compartments(&self) -> &Compartments { &self.0 }
            fn roles(&self) -> Option<&Roles> { None }
        }
        let session = SessionId::new();
        let (own, other) = (place(session, Confidential, &[]), place(session, Public, &[]));
        let policy = DenyAll(Compartments::default());
        assert!(policy.may_copy_information(&someone(), &own, &own), "a channel into itself, without asking");
        assert!(!policy.may_copy_information(&someone(), &other, &own), "another channel as the policy decides");
    }

    #[test]
    fn what_set_a_label_is_told_by_level_by_audience_and_by_what_is_not_known() {
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let (internal, confidential) = (Classification::general(Internal), Classification::general(Confidential));

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        assert!(causes.set_by(Some(&confidential)), "a part at the label's level set it");
        assert!(!causes.set_by(Some(&internal)), "one below did not");
        assert!(causes.set_by(None), "what is not known counted as the channel");

        let sales = classified(Internal, &["sales"]);
        let causes = LabelCauses::new(confidential.clone(), [&sales, &classified(Internal, &["hr"])]);
        assert!(causes.set_by(Some(&sales)), "without an audience in common, the parts with one set the channel's label");
        assert!(!causes.set_by(Some(&internal)), "a general part did not");

        let causes = LabelCauses::new(confidential.clone(), [&internal, &confidential]);
        let result = ToolCallResult::of(
            &ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() },
            Default::default(), "secret".to_string(), confidential.clone());
        let message = Message { tool_results: vec![result], ..Message::new(internal.clone()) };
        assert!(causes.set_by_message(&message), "a tool result in a message sets the label as the message");
        assert!(!causes.set_by_message(&Message::new(internal)));
    }
}
