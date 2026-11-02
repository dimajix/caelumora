//! What tests decide by: the policies a test opens the stores by (`Policies::permitting_all`,
//! `Policies::by_default_rules`, issue 449), for the tests of this crate and of others; and what
//! this crate's tests share (`fixtures`).

#[cfg(test)]
pub(crate) use fixtures::*;

use crate::confidentiality::ConfidentialityRules;
use crate::{AccessPolicy, AllowAllPolicy, Policies};
use std::sync::Arc;

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

/// What the tests of this crate share: actors, their contexts, attributes that share an entity,
/// the groups they belong to, and what the ownership policy answers.
#[cfg(test)]
mod fixtures {
    pub(crate) use caelumora_domain::security::testing::{shared, user};

    use crate::{AccessPolicy, ActionType, Memberships, OwnershipPolicy, SecurityContext};
    use async_trait::async_trait;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::removal::Removal;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::ScopedName;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) fn context(actor: ActorId) -> SecurityContext {
        SecurityContext::new(actor, SessionId::default(), ChannelId::default())
    }

    pub(crate) async fn allows(actor: ActorId, action: ActionType<'_>) -> bool {
        OwnershipPolicy::new().check(&context(actor), action).await
    }

    /// Memberships as a test sets them: who belongs to which group, and which
    /// groups were deleted. Every group asked about is there, if only empty.
    #[derive(Debug, Default)]
    pub(crate) struct Known {
        pub(crate) members: Vec<(ActorId, GroupId)>,
        pub(crate) deleted: Vec<GroupId>,
    }

    #[async_trait]
    impl Memberships for Known {
        async fn group(&self, id: GroupId) -> Option<Group> {
            let members = self
                .members
                .iter()
                .filter(|(_, group)| *group == id)
                .map(|(actor, _)| *actor)
                .collect();
            let deleted = self.deleted.contains(&id).then(|| Removal::now(ActorId::System));
            let name = ScopedName::new("known", Scope::GlobalScope).ok()?;
            Some(Group { id, members, deleted, ..Group::team(name, UserId::new()) })
        }
    }

    /// `Known` memberships that count how often a group is asked for (issue
    /// 503).
    #[derive(Debug)]
    pub(crate) struct Counted {
        known: Known,
        asked: AtomicUsize,
    }

    impl Counted {
        pub(crate) fn new(known: Known) -> Self {
            Self { known, asked: AtomicUsize::new(0) }
        }

        /// How often a group was asked for so far.
        pub(crate) fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Memberships for Counted {
        async fn group(&self, id: GroupId) -> Option<Group> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.known.group(id).await
        }
    }
}
