//! `Policies`: the `AccessPolicy` and the `ConfidentialityPolicy` of a process, as one pair.

use super::AccessPolicy;
#[cfg(any(test, feature = "test-support"))]
use super::AllowAllPolicy;
use crate::confidentiality::ConfidentialityPolicy;
#[cfg(any(test, feature = "test-support"))]
use crate::confidentiality::ConfidentialityRules;
use std::sync::Arc;

/// The `AccessPolicy` and the `ConfidentialityPolicy` of the process (issue 449): what the kernel
/// is given, opens its stores by and hands to whatever asks both - the event bus, the loops, the
/// views, the managers and controllers that check.
#[derive(Debug, Clone)]
pub struct Policies {
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl Policies {
    /// The policies of `access_policy`, which decides who may do what, and `confidentiality`,
    /// which decides what may flow where and who is cleared for what.
    pub fn new(
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self { access_policy, confidentiality }
    }

    /// Who may do what.
    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        &self.access_policy
    }

    /// What may flow where, and who is cleared for what (issue 304).
    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> {
        &self.confidentiality
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Policies {
    /// The policies that permit everything (`AllowAllPolicy`) by the default rules - for what a
    /// test writes before it runs what it tests, and for what a test runs that checks nothing.
    pub fn permitting_all() -> Self {
        Self::by_default_rules(Arc::new(AllowAllPolicy::new()))
    }

    /// The policies of `access_policy` by the default rules (`ConfidentialityRules::default`) - for
    /// a test of what `access_policy` decides.
    pub fn by_default_rules(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self::new(access_policy, Arc::new(ConfidentialityRules::default()))
    }
}
