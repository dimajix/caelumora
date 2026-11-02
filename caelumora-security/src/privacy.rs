//! The policies of privacy between sessions (issue 134): whom a session lets read what was said in
//! it (`PrivacyPolicy`), one for each policy a session may choose (`PrivacyConfig`). What a session
//! is to privacy is the domain's (`caelumora_domain::privacy`).
//!
//! A policy has to be transitive: what it lets another session read, that session passes on by its
//! own policy, and together they must not break the first one's promise. `SameOwner` and
//! `SharedAudience` are, also mixed; a grant to a session named by the owner would not be - that
//! session's owner could pass it on to anyone.

use caelumora_domain::actor::ActorId;
use caelumora_domain::privacy::{PrivacyConfig, SessionPrivacy};
use std::fmt::Debug;

/// Whom a session lets read what was said in it. Asked only across
/// sessions, of the session read from.
pub(crate) trait PrivacyPolicy: Debug + Send + Sync {
    /// Whether the session `reader` may read what was said in `source`.
    fn permits(&self, source: &SessionPrivacy, reader: &SessionPrivacy) -> bool;
}

impl dyn PrivacyPolicy {
    /// The policy `config` chooses.
    pub(crate) fn of(config: PrivacyConfig) -> &'static dyn PrivacyPolicy {
        match config {
            PrivacyConfig::Private => &Private,
            PrivacyConfig::SameOwner => &SameOwner,
            PrivacyConfig::SharedAudience => &SharedAudience,
        }
    }
}

/// [`PrivacyConfig::Private`].
#[derive(Debug)]
struct Private;

impl PrivacyPolicy for Private {
    fn permits(&self, _source: &SessionPrivacy, _reader: &SessionPrivacy) -> bool {
        false
    }
}

/// [`PrivacyConfig::SameOwner`].
#[derive(Debug)]
struct SameOwner;

impl PrivacyPolicy for SameOwner {
    fn permits(&self, source: &SessionPrivacy, reader: &SessionPrivacy) -> bool {
        // Only a user owns: two sessions without an owner, or of the
        // system, are not one person's.
        matches!(source.owner, ActorId::User(_)) && source.owner == reader.owner
    }
}

/// [`PrivacyConfig::SharedAudience`].
#[derive(Debug)]
struct SharedAudience;

impl PrivacyPolicy for SharedAudience {
    fn permits(&self, source: &SessionPrivacy, reader: &SessionPrivacy) -> bool {
        reader.people.is_subset(&source.people)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;

    /// Whether `reader` may read what was said in `source`, as `source`'s policy decides.
    fn permits(source: &SessionPrivacy, reader: &SessionPrivacy) -> bool {
        <dyn PrivacyPolicy>::of(source.policy).permits(source, reader)
    }

    fn session(policy: PrivacyConfig, owner: UserId, others: &[UserId]) -> SessionPrivacy {
        let people = others.iter().copied().chain([owner]).collect();
        SessionPrivacy { id: SessionId::new(), policy, owner: ActorId::User(owner), people }
    }

    #[test]
    fn a_private_session_is_read_by_no_other() {
        let alice = UserId::new();
        let source = session(PrivacyConfig::Private, alice, &[]);
        let reader = session(PrivacyConfig::Private, alice, &[]);
        assert!(!permits(&source, &reader), "not even by its owner's");
    }

    #[test]
    fn same_owner_lets_the_owners_other_sessions_read() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let source = session(PrivacyConfig::SameOwner, alice, &[bob]);
        assert!(
            permits(&source, &session(PrivacyConfig::Private, alice, &[])),
            "whatever the reader's own policy"
        );
        assert!(
            !permits(&source, &session(PrivacyConfig::Private, bob, &[alice])),
            "not a participant's"
        );
        let unowned =
            SessionPrivacy { policy: PrivacyConfig::SameOwner, ..SessionPrivacy::default() };
        assert!(
            !permits(&unowned, &SessionPrivacy::default()),
            "nobody's sessions are not one person's"
        );
    }

    #[test]
    fn shared_audience_lets_read_who_took_part_anyway() {
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let source = session(PrivacyConfig::SharedAudience, alice, &[bob]);
        assert!(
            permits(&source, &session(PrivacyConfig::Private, bob, &[])),
            "a participant's own session"
        );
        assert!(
            permits(&source, &session(PrivacyConfig::Private, alice, &[bob])),
            "the same people"
        );
        assert!(
            !permits(&source, &session(PrivacyConfig::Private, alice, &[carol])),
            "not with someone who was not there"
        );
    }
}
