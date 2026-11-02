//! The flow decisions (issues 133, 304): whether information may go where an act takes it, and
//! whether someone is cleared for it (`ConfidentialityPolicy`, asked as
//! `ConfidentialityQuestions`), and the rules Caelumora applies (`ConfidentialityRules`). What a
//! classification is, and the one flow rule over classifications, are the domain's
//! (`caelumora_domain::confidentiality`).

use crate::privacy::PrivacyPolicy;
use crate::{ActionType, Policies, SecurityContext};
use caelumora_domain::actor::ActorId;
use caelumora_domain::confidentiality::{Classification, Compartments, Place, Reading};
use caelumora_domain::config::AppConfig;
use caelumora_domain::privacy::SessionPrivacy;
use caelumora_domain::role::{RoleId, Roles};
use caelumora_domain::session::ChannelId;
use std::fmt;

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
    fn may_publish(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
    ) -> bool;

    /// What of the channel `channel`, in a session with the privacy
    /// `session`, may be copied, in `context`, to `into`, where an agent
    /// reads it (issues 17, 134, 264): not a decision for each
    /// classification, but which classifications - data, so that a store
    /// reads only what may be copied (issue 421).
    fn copyable(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        session: &SessionPrivacy,
        into: &Place,
    ) -> Reading;

    /// Whether `participant`, by `roles`, is cleared for what is classified
    /// `classification` - asked in `context`, which may be another's: whoever
    /// admits the participant.
    fn cleared(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        roles: &[RoleId],
        classification: &Classification,
    ) -> bool;

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
    fn copyable_information(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        session: &SessionPrivacy,
        into: &Place,
    ) -> Reading;

    /// Whether what is at `from` may be copied to `into` in `context`
    /// (`copyable_information`).
    fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool;

    /// Whether `context` is cleared for `classification` by the roles of its
    /// owner, who answers for the caller - and, for a caller limited to
    /// roles, by those as well (issues 179, 341): what decides which
    /// channels, and what else classified, a viewer is shown.
    /// The system itself (`SecurityContext::is_system`) is cleared for
    /// everything, as it may do everything (issues 312, 477).
    fn cleared_for(&self, context: &SecurityContext, classification: &Classification) -> bool;

    /// Nothing if what is classified `classification` may be put into a
    /// destination classified `into`; `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if it may not.
    fn require_publish<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E>;

    /// Nothing if what is so classified may be put into a storage bounded
    /// by `destination` - a storage of workspaces or of content, which says
    /// how much what lies there may be classified at most -, as publishing
    /// it there (`require_publish`); a storage without one takes anything.
    ///
    /// # Errors
    /// `refusal()` if it may not.
    fn require_storable<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        destination: Option<&Classification>,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E>;

    /// Nothing if `participant`, by `roles`, is cleared for `classification`;
    /// `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if they are not.
    fn require_cleared<E>(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        roles: &[RoleId],
        classification: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E>;

    /// Nothing if `context` is cleared for `classification` (`cleared_for`);
    /// `refusal` otherwise.
    ///
    /// # Errors
    /// `refusal()` if it is not.
    fn require_cleared_for<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E>;

    /// Nothing if `classification` names only configured compartments;
    /// `invalid` otherwise - a classification that cannot be given at all,
    /// which no permission changes.
    ///
    /// # Errors
    /// `invalid()` if it is not.
    fn require_classifiable<E>(
        &self,
        classification: &Classification,
        invalid: impl FnOnce() -> E,
    ) -> Result<(), E>;
}

impl<T: ConfidentialityPolicy + ?Sized> ConfidentialityQuestions for T {
    fn copyable_information(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        session: &SessionPrivacy,
        into: &Place,
    ) -> Reading {
        match channel == into.channel {
            true => Reading::Whole,
            false => self.copyable(context, channel, session, into),
        }
    }

    fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool {
        self.copyable_information(context, from.channel, &from.session, into)
            .admits(&from.classification, self.compartments())
    }

    fn cleared_for(&self, context: &SecurityContext, classification: &Classification) -> bool {
        if context.is_system() {
            return true;
        }
        let participant = context.owner_id;
        self.cleared(context, participant, &context.roles, classification)
            && context
                .limiting_roles
                .as_ref()
                .is_none_or(|limit| self.cleared(context, participant, limit, classification))
    }

    fn require_publish<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E> {
        refused(
            self.may_publish(context, classification, into),
            context,
            || format!("publishing {classification} into {into}"),
            refusal,
        )
    }

    fn require_storable<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        destination: Option<&Classification>,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E> {
        destination.map_or(Ok(()), |destination| {
            self.require_publish(context, classification, destination, refusal)
        })
    }

    fn require_cleared<E>(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        roles: &[RoleId],
        classification: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E> {
        refused(
            self.cleared(context, participant, roles, classification),
            context,
            || format!("{participant} not cleared for {classification}"),
            refusal,
        )
    }

    fn require_cleared_for<E>(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E> {
        refused(
            self.cleared_for(context, classification),
            context,
            || format!("not cleared for {classification}"),
            refusal,
        )
    }

    fn require_classifiable<E>(
        &self,
        classification: &Classification,
        invalid: impl FnOnce() -> E,
    ) -> Result<(), E> {
        match self.compartments().check(classification) {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::debug!("Refused to classify as {classification}: {e}");
                Err(invalid())
            }
        }
    }
}

/// Whether `context` reaches what `read` is asked of and is classified as `classifications` says
/// (concept 4 of the permission checks, issue 322): it may read it (the access policy of
/// `policies`), and, where it is classified, is cleared for one of them (its confidentiality
/// policy) - one, the entity's; two as it is reclassified, the one it had and the one it has. The
/// question the store asks of what it lists, and - as `may_reach_each`, which answers alike - the
/// event bus of whom an event reaches, so that both reach alike. A question, for filtering: an
/// operation reads through the store, which enforces it.
pub async fn may_reach(
    policies: &Policies,
    context: &SecurityContext,
    read: ActionType<'_>,
    classifications: &[Classification],
) -> bool {
    policies.access_policy().check(context, read).await
        && cleared(policies.confidentiality().as_ref(), context, classifications)
}

/// `may_reach` for each of `contexts` at once, in their order, each
/// answered as `may_reach` answers it: the read question asked of all of
/// them together (`AccessPolicy::check_each`), as the event bus asks it of
/// every party whom an event reaches (issue 503).
pub async fn may_reach_each(
    policies: &Policies,
    contexts: &[SecurityContext],
    read: ActionType<'_>,
    classifications: &[Classification],
) -> Vec<bool> {
    let readable = policies.access_policy().check_each(contexts, read).await;
    let confidentiality = policies.confidentiality().as_ref();
    contexts
        .iter()
        .zip(readable)
        .map(|(context, readable)| readable && cleared(confidentiality, context, classifications))
        .collect()
}

/// Whether `context` is cleared for one of `classifications`, or they are
/// none: the second half of `may_reach`.
fn cleared(
    confidentiality: &dyn ConfidentialityPolicy,
    context: &SecurityContext,
    classifications: &[Classification],
) -> bool {
    classifications.is_empty()
        || classifications
            .iter()
            .any(|classification| confidentiality.cleared_for(context, classification))
}

/// `Ok` if `allowed`, else `refusal()`, logged at `debug!` by what
/// `describe` names - never content.
fn refused<E>(
    allowed: bool,
    context: &SecurityContext,
    describe: impl FnOnce() -> String,
    refusal: impl FnOnce() -> E,
) -> Result<(), E> {
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

    /// The rules `config` configures (issue 304): its compartments, checking clearances by its
    /// roles if it configures any.
    pub fn configured(config: &AppConfig) -> Self {
        let rules = Self::new(config.compartments.clone());
        match &config.roles {
            Some(roles) => rules.with_roles(roles.clone()),
            None => rules,
        }
    }
}

impl ConfidentialityPolicy for ConfidentialityRules {
    /// If it names only configured compartments and flows into `into`.
    fn may_publish(
        &self,
        _context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        self.compartments.check(classification).is_ok()
            && classification.flows_to(into, &self.compartments)
    }

    /// A channel into itself the whole; within a session what flows into
    /// `into`; beyond it only where the session of the channel permits it
    /// as well (issue 134), and nothing otherwise - whatever the
    /// classification, `public` included.
    fn copyable(
        &self,
        _context: &SecurityContext,
        channel: ChannelId,
        session: &SessionPrivacy,
        into: &Place,
    ) -> Reading {
        if channel == into.channel {
            return Reading::Whole;
        }
        match session.id == into.session.id
            || <dyn PrivacyPolicy>::of(session.policy).permits(session, &into.session)
        {
            true => Reading::Released(into.classification.clone()),
            false => Reading::Nothing,
        }
    }

    /// By the clearances the configured roles grant. Without roles
    /// configured, everybody is.
    fn cleared(
        &self,
        _context: &SecurityContext,
        _participant: ActorId,
        roles: &[RoleId],
        classification: &Classification,
    ) -> bool {
        match &self.roles {
            Some(configured) => {
                configured.clearances_of(roles).admits(classification, &self.compartments)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OwnershipPolicy;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::confidentiality::testing::{classified, compartment, organisation};
    use caelumora_domain::confidentiality::Clearance;
    use caelumora_domain::confidentiality::Confidentiality::{self, *};
    use caelumora_domain::security::SecurityAttributes;
    use caelumora_domain::session::{Session, SessionId};
    use caelumora_domain::store::Stored;
    use caelumora_domain::user::UserId;
    use std::sync::Arc;

    #[test]
    fn the_rules_give_only_configured_compartments_and_check_clearances_only_with_roles() {
        let rules = ConfidentialityRules::new(organisation());
        let anyone = SecurityContext::default();
        let classifiable = |c: &Classification| rules.require_classifiable(c, || ()).is_ok();
        assert!(
            classifiable(&classified(Internal, &["hr"]))
                && !classifiable(&classified(Internal, &["hrr"]))
        );
        assert!(rules.may_publish(
            &anyone,
            &classified(Internal, &["hr"]),
            &classified(Confidential, &["hr_management"])
        ));
        assert!(
            !rules.may_publish(
                &anyone,
                &classified(Internal, &["hrr"]),
                &classified(Internal, &["hrr"])
            ),
            "not configured, though it would flow"
        );
        assert!(
            !rules.may_publish(
                &anyone,
                &classified(Confidential, &["hr"]),
                &classified(Internal, &["hr"])
            ),
            "it would not flow"
        );

        let secret = classified(Confidential, &["hr"]);
        let nobody = ActorId::None;
        assert!(
            rules.cleared(&anyone, nobody, &[], &secret),
            "without roles, clearances are not checked"
        );
        let hr = RoleId::new();
        let roles = Roles::new([(
            hr,
            "hr".to_string(),
            vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }],
        )])
        .unwrap();
        let rules = rules.with_roles(roles);
        assert_eq!(rules.roles().and_then(|r| r.name_of(hr)), Some("hr"));
        assert!(rules.cleared(&anyone, nobody, &[hr], &secret));
        assert!(
            !rules.cleared(&anyone, nobody, &[], &secret)
                && rules.cleared(&anyone, nobody, &[], &Classification::general(Public)),
            "with roles, nobody else but for what is public"
        );
    }

    /// Issue 503: asked of several contexts at once, each reaches as it does
    /// alone - read, and cleared for what it is.
    #[tokio::test]
    async fn reaching_asked_of_many_at_once_is_reaching_asked_of_each() {
        let hr = RoleId::new();
        let roles = Roles::new([(
            hr,
            "hr".to_string(),
            vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(organisation()).with_roles(roles);
        let owner = ActorId::User(UserId::new());
        let attributes = SecurityAttributes::owned_by(owner);
        let secret = [classified(Confidential, &["hr"])];
        let as_owner = SecurityContext::new(owner, Default::default(), Default::default());
        let contexts = [
            as_owner.clone().with_roles(vec![hr], None),
            as_owner,
            someone().with_roles(vec![hr], None),
        ];
        let policies = Policies::new(Arc::new(OwnershipPolicy::new()), Arc::new(rules));
        let read = ActionType::get(Session::KIND, &attributes);

        let at_once = may_reach_each(&policies, &contexts, read, &secret).await;
        let mut alone = Vec::new();
        for context in &contexts {
            alone.push(may_reach(&policies, context, read, &secret).await);
        }
        assert_eq!(at_once, alone);
        assert_eq!(at_once, [true, false, false], "read and cleared; not cleared; not read");
    }

    /// Issue 304: a viewer is cleared by the roles of their context, and a
    /// caller limited to roles by those as well.
    #[test]
    fn a_context_is_cleared_by_its_roles_and_its_limit() {
        let hr = RoleId::new();
        let roles = Roles::new([(
            hr,
            "hr".to_string(),
            vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(organisation()).with_roles(roles);
        let secret = classified(Confidential, &["hr"]);
        let alice = SecurityContext::new(
            ActorId::User(UserId::new()),
            Default::default(),
            Default::default(),
        );
        assert!(!rules.cleared_for(&alice, &secret), "without roles");
        let hers = alice.clone().with_roles(vec![hr], None);
        assert!(rules.cleared_for(&hers, &secret));
        assert!(
            !rules.cleared_for(&alice.with_roles(vec![hr], Some(vec![])), &secret),
            "limited below it"
        );
        assert!(rules.require_cleared_for(&hers, &secret, || ()).is_ok());
    }

    /// Issue 341: a caller is cleared by its owner, who answers for it -
    /// whoever it acts for lends no clearance, not even the system. Only the
    /// system itself is cleared for everything (issue 477).
    #[test]
    fn a_caller_is_cleared_as_its_owner_whoever_it_acts_for() {
        let hr = RoleId::new();
        let roles = Roles::new([(
            hr,
            "hr".to_string(),
            vec![Clearance { level: Confidential, compartment: Some(compartment("hr")) }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(organisation()).with_roles(roles);
        let secret = classified(Confidential, &["hr"]);
        let owner = ActorId::User(UserId::new());
        let agent = ActorId::Agent(AgentId::new());
        let for_the_system = SecurityContext::system().next(agent, owner, ActorId::System);
        assert!(
            !rules.cleared_for(&for_the_system, &secret),
            "acting for the system, still bounded by its owner"
        );
        let owned_by_the_system = SecurityContext::system().next(agent, ActorId::System, owner);
        assert!(
            !rules.cleared_for(&owned_by_the_system, &secret),
            "an agent the system answers for is not the system"
        );
        assert!(rules.cleared_for(&SecurityContext::system(), &secret), "the system itself is");
    }

    /// A channel of the session `session_id`, a private one of nobody.
    fn place(session_id: SessionId, level: Confidentiality, compartments: &[&str]) -> Place {
        in_session(
            SessionPrivacy { id: session_id, ..SessionPrivacy::default() },
            level,
            compartments,
        )
    }

    fn in_session(session: SessionPrivacy, level: Confidentiality, compartments: &[&str]) -> Place {
        Place {
            channel: ChannelId::new(),
            classification: classified(level, compartments),
            session,
        }
    }

    fn hr_reads_finance() -> ConfidentialityRules {
        ConfidentialityRules::new(
            Compartments::new([
                (compartment("hr"), vec![compartment("finance")]),
                (compartment("finance"), vec![]),
            ])
            .unwrap(),
        )
    }

    fn someone() -> SecurityContext {
        SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default())
    }

    #[test]
    fn reading_a_channel_depends_on_the_channels_not_on_the_reader() {
        let rules = ConfidentialityRules::default();
        let session = SessionId::new();
        let (team, secret) = (place(session, Internal, &[]), place(session, Confidential, &[]));
        for reader in [someone(), someone()] {
            assert!(rules.may_copy_information(&reader, &team, &secret));
            assert!(!rules.may_copy_information(&reader, &secret, &team));
        }
    }

    #[test]
    fn reading_a_channel_follows_the_flow_rule_with_the_configured_compartments() {
        let rules = hr_reads_finance();
        let session = SessionId::new();
        let alice = someone();
        let hr = place(session, Internal, &["hr"]);
        let finance = place(session, Internal, &["finance"]);
        let general = place(session, Internal, &[]);
        assert!(rules.may_copy_information(&alice, &finance, &hr), "HR reads finance");
        assert!(!rules.may_copy_information(&alice, &hr, &finance), "not the other way round");
        assert!(!rules.may_copy_information(&alice, &hr, &general), "nor a general channel");
        assert!(
            rules.may_copy_information(&alice, &general, &hr),
            "what is general flows anywhere"
        );
    }

    #[test]
    fn a_channel_reads_another_session_only_where_that_session_permits_it() {
        use caelumora_domain::privacy::PrivacyConfig;
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
        let agent = SecurityContext::new(
            ActorId::Agent(AgentId::new()),
            Default::default(),
            Default::default(),
        );
        assert!(
            !rules.may_copy_information(
                &agent,
                &elsewhere(PrivacyConfig::Private, Internal),
                &reader
            ),
            "a private session is read by no other"
        );
        assert!(
            !rules.may_copy_information(
                &agent,
                &elsewhere(PrivacyConfig::Private, Public),
                &reader
            ),
            "not even what is public"
        );
        assert!(
            rules.may_copy_information(
                &agent,
                &elsewhere(PrivacyConfig::SameOwner, Internal),
                &reader
            ),
            "her other sessions, where she lets them"
        );
        assert!(
            rules.may_copy_information(
                &agent,
                &elsewhere(PrivacyConfig::SharedAudience, Internal),
                &reader
            ),
            "she took part there"
        );
        assert!(
            !rules.may_copy_information(
                &agent,
                &elsewhere(PrivacyConfig::SameOwner, Confidential),
                &reader
            ),
            "and still only what flows into the reader"
        );
    }

    #[test]
    fn what_is_released_below_its_channel_is_read_where_the_channel_is_not() {
        let rules = hr_reads_finance();
        let session = SessionId::new();
        let alice = someone();
        let hr = place(session, Confidential, &["hr"]);
        let finance = place(session, Confidential, &["finance"]);
        assert!(!rules.may_copy_information(&alice, &hr, &finance), "finance does not read HR");
        let released =
            Place { classification: classified(Confidential, &["hr", "finance"]), ..hr.clone() };
        assert!(
            rules.may_copy_information(&alice, &released, &finance),
            "but what HR released to it"
        );
        let elsewhere = place(SessionId::new(), Confidential, &["finance"]);
        assert!(
            !rules.may_copy_information(&alice, &released, &elsewhere),
            "not from another session"
        );
    }

    #[test]
    fn what_is_published_into_a_channel_has_to_flow_into_it() {
        let rules = hr_reads_finance();
        let alice = someone();
        let hr = classified(Confidential, &["hr"]);
        assert!(rules.may_publish(&alice, &hr, &hr), "the channel's own");
        assert!(
            rules.may_publish(&alice, &classified(Internal, &["hr", "finance"]), &hr),
            "released to finance, at a lower level"
        );
        assert!(
            !rules.may_publish(&alice, &classified(StrictlyConfidential, &["hr"]), &hr),
            "above the channel - never reclassified there either"
        );
        assert!(
            rules.may_publish(&alice, &classified(Confidential, &["finance"]), &hr),
            "HR may read what is finance's"
        );
        assert!(
            rules.may_publish(&alice, &Classification::general(Confidential), &hr),
            "what is general flows into any channel"
        );
        assert!(
            !rules.may_publish(&alice, &classified(Confidential, &["hr", "payroll"]), &hr),
            "not configured"
        );
    }

    #[test]
    fn a_channel_is_copied_into_itself_whatever_the_policy() {
        /// Refuses every flow.
        #[derive(Debug)]
        struct DenyAll(Compartments);
        impl ConfidentialityPolicy for DenyAll {
            fn may_publish(
                &self,
                _: &SecurityContext,
                _: &Classification,
                _: &Classification,
            ) -> bool {
                false
            }
            fn copyable(
                &self,
                _: &SecurityContext,
                _: ChannelId,
                _: &SessionPrivacy,
                _: &Place,
            ) -> Reading {
                Reading::Nothing
            }
            fn cleared(
                &self,
                _: &SecurityContext,
                _: ActorId,
                _: &[RoleId],
                _: &Classification,
            ) -> bool {
                false
            }
            fn compartments(&self) -> &Compartments {
                &self.0
            }
            fn roles(&self) -> Option<&Roles> {
                None
            }
        }
        let session = SessionId::new();
        let (own, other) = (place(session, Confidential, &[]), place(session, Public, &[]));
        let policy = DenyAll(Compartments::default());
        assert!(
            policy.may_copy_information(&someone(), &own, &own),
            "a channel into itself, without asking"
        );
        assert!(
            !policy.may_copy_information(&someone(), &other, &own),
            "another channel as the policy decides"
        );
    }
}
