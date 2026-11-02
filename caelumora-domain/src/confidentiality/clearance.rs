//! Who may see what (issue 133): the clearances of all of someone's roles (`Clearance`,
//! `Clearances`), judged by a rule of their own rather than by the flow rule.

use super::{Classification, Compartment, Compartments, Confidentiality};
use serde::{Deserialize, Serialize};

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
        if classification.level() > self.level {
            return false;
        }
        match &self.compartment {
            None => classification.compartments().is_empty(),
            Some(own) => {
                classification.compartments().iter().any(|c| compartments.may_read(own, c))
            }
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
        classification.level() == Confidentiality::Public
            || self.0.iter().any(|c| c.sees(classification, compartments))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::testing::{classified, compartment, organisation};
    use Confidentiality::*;

    fn compartment_named(name: &str) -> Compartment {
        compartment(name)
    }

    #[test]
    fn a_clearance_sees_per_compartment_up_to_its_own_level() {
        let compartments = organisation();
        let clearance = |level, compartment: Option<&str>| Clearance {
            level,
            compartment: compartment.map(compartment_named),
        };
        // Alice: internal of sales, confidential of HR, and what is general up to internal.
        let alice = Clearances(vec![
            clearance(Internal, Some("sales")),
            clearance(Confidential, Some("hr")),
            clearance(Internal, None),
        ]);
        assert!(alice.admits(&classified(Confidential, &["hr"]), &compartments));
        assert!(
            !alice.admits(&classified(Confidential, &["sales"]), &compartments),
            "only internal of sales"
        );
        assert!(alice.admits(&classified(Internal, &["sales"]), &compartments));
        assert!(
            alice.admits(&classified(Confidential, &["finance"]), &compartments),
            "HR reads finance"
        );
        assert!(
            alice.admits(&classified(Confidential, &["hr", "sales"]), &compartments),
            "published to HR too"
        );
        assert!(
            !alice.admits(&classified(Internal, &["hr_management"]), &compartments),
            "not hers"
        );
        assert!(alice.admits(&Classification::general(Internal), &compartments));
        assert!(
            !alice.admits(&Classification::general(Confidential), &compartments),
            "a compartment says nothing of what is general"
        );
        assert!(
            Clearances::default().admits(&Classification::general(Public), &compartments),
            "public is public"
        );
        assert!(
            !Clearances::default().admits(&Classification::general(Internal), &compartments),
            "without a clearance only that"
        );
        let management = Clearances(vec![clearance(Internal, Some("hr_management"))]);
        assert!(
            management.admits(&classified(Internal, &["finance"]), &compartments),
            "through what it reads"
        );
    }
}
