//! `Policies`: the `AccessPolicy` and the `ConfidentialityPolicy` of a process, as one pair.

use crate::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use crate::{AccessPolicy, AllowAllPolicy, Memberships, OwnershipPolicy};
use caelumora_model::config::{AppConfig, SecurityConfig};
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

    /// The policies `config` chooses (issues 11, 304): the access policy it names, learning who
    /// belongs to a group from `memberships` (issues 23, 286), and its rules of confidentiality
    /// (`ConfidentialityRules::configured`) - also under `allow_all`, which allows every action but
    /// no flow the rules refuse.
    pub fn configured(config: &AppConfig, memberships: Arc<dyn Memberships>) -> Self {
        let access_policy: Arc<dyn AccessPolicy> = match config.security {
            SecurityConfig::Ownership => Arc::new(OwnershipPolicy::with_memberships(memberships)),
            SecurityConfig::AllowAll => {
                tracing::warn!(
                    "Security policy 'allow_all' is configured: every user may do everything. \
                     Development only."
                );
                Arc::new(AllowAllPolicy::new())
            }
        };
        Self::new(access_policy, Arc::new(ConfidentialityRules::configured(config)))
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
