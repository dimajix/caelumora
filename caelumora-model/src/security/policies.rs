//! `Policies`: the `AccessPolicy` and the `ConfidentialityPolicy` of a process, as one pair.

use super::AccessPolicy;
use crate::confidentiality::ConfidentialityPolicy;
use std::sync::Arc;

/// The `AccessPolicy` and the `ConfidentialityPolicy` of the process (issue 449): what the runtime
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
