//! Privacy between sessions (issue 134): whom a session lets read what was
//! said in it.
//!
//! Confidentiality (`crate::confidentiality`) decides by how sensitive
//! something is what *may* be shared; privacy decides whether the
//! participants of a session *agreed*. Within a session, its participants
//! share with each other and only the classifications decide. Beyond it,
//! the source session's privacy policy has to permit it as well.
//!
//! Every session has exactly one policy, chosen by the [`PrivacyConfig`]
//! stored with it. A policy has to be transitive: what it lets another
//! session read, that session passes on by its own policy, and together
//! they must not break the first one's promise. `SameOwner` and
//! `SharedAudience` are, also mixed; a grant to a session named by the
//! owner would not be - that session's owner could pass it on to anyone.

use crate::actor::ActorId;
use crate::session::{Participant, Session};
use crate::user::UserId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Debug;

/// Which privacy policy a session has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyConfig {
    /// What was said in the session is read only within it.
    #[default]
    Private,
    /// Other sessions of the same owner may read it.
    SameOwner,
    /// Other sessions may read it whose people all take part in this one:
    /// nobody learns anything they could not have heard here.
    SharedAudience,
}

impl PrivacyConfig {
    /// The policy this configuration chooses.
    pub fn policy(self) -> &'static dyn PrivacyPolicy {
        match self {
            PrivacyConfig::Private => &Private,
            PrivacyConfig::SameOwner => &SameOwner,
            PrivacyConfig::SharedAudience => &SharedAudience,
        }
    }
}

/// Whom a session lets read what was said in it. Asked only across
/// sessions, of the session read from.
pub trait PrivacyPolicy: Debug + Send + Sync {
    /// Whether the session `reader` may read what was said in `source`.
    fn permits(&self, source: &SessionPrivacy, reader: &SessionPrivacy) -> bool;
}

/// A session as privacy sees it: its policy, its owner, and its people.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPrivacy {
    pub policy: PrivacyConfig,
    pub owner: ActorId,
    /// The users who take part in the session, its owner included. Agents
    /// and approvers are not counted: what an agent learns, it passes on
    /// only in its sessions, which are asked in turn.
    pub people: BTreeSet<UserId>,
}

impl SessionPrivacy {
    /// `session` as privacy sees it.
    pub fn of(session: &Session) -> Self {
        let owner = session.security_attributes.owner_id;
        let people = session.participants.iter()
            .filter_map(|p| match p {
                Participant::User(user) => Some(*user),
                _ => None,
            })
            .chain(owner.as_user_id())
            .collect();
        Self { policy: session.privacy, owner, people }
    }

    /// Whether the session `reader` may read what was said in this one, as
    /// this one's policy decides.
    pub fn permits(&self, reader: &SessionPrivacy) -> bool {
        self.policy.policy().permits(self, reader)
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

    fn session(policy: PrivacyConfig, owner: UserId, others: &[UserId]) -> SessionPrivacy {
        let people = others.iter().copied().chain([owner]).collect();
        SessionPrivacy { policy, owner: ActorId::User(owner), people }
    }

    #[test]
    fn a_private_session_is_read_by_no_other() {
        let alice = UserId::new();
        let (source, reader) = (session(PrivacyConfig::Private, alice, &[]), session(PrivacyConfig::Private, alice, &[]));
        assert!(!source.permits(&reader), "not even by its owner's");
        assert_eq!(PrivacyConfig::default(), PrivacyConfig::Private, "the default");
        let names: Vec<_> = [PrivacyConfig::Private, PrivacyConfig::SameOwner, PrivacyConfig::SharedAudience].iter().map(|p| serde_json::to_value(p).unwrap()).collect();
        assert_eq!(names, ["private", "same_owner", "shared_audience"], "as the API and the web UI name them");
    }

    #[test]
    fn same_owner_lets_the_owners_other_sessions_read() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let source = session(PrivacyConfig::SameOwner, alice, &[bob]);
        assert!(source.permits(&session(PrivacyConfig::Private, alice, &[])), "whatever the reader's own policy");
        assert!(!source.permits(&session(PrivacyConfig::Private, bob, &[alice])), "not a participant's");
        let unowned = SessionPrivacy { policy: PrivacyConfig::SameOwner, ..SessionPrivacy::default() };
        assert!(!unowned.permits(&SessionPrivacy::default()), "nobody's sessions are not one person's");
    }

    #[test]
    fn shared_audience_lets_read_who_took_part_anyway() {
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let source = session(PrivacyConfig::SharedAudience, alice, &[bob]);
        assert!(source.permits(&session(PrivacyConfig::Private, bob, &[])), "a participant's own session");
        assert!(source.permits(&session(PrivacyConfig::Private, alice, &[bob])), "the same people");
        assert!(!source.permits(&session(PrivacyConfig::Private, alice, &[carol])), "not with someone who was not there");
    }

    #[test]
    fn the_people_of_a_session_are_its_users_and_its_owner() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let session = Session {
            id: Default::default(),
            name: String::new(),
            participants: vec![Participant::User(bob), Participant::Agent(Default::default())],
            channels: Vec::new(),
            commands: Default::default(),
            privacy: PrivacyConfig::SharedAudience,
            created_at: Default::default(),
            updated_at: Default::default(),
            security_attributes: crate::security::SecurityAttributes::owned_by(ActorId::User(alice)),
        };
        let privacy = SessionPrivacy::of(&session);
        assert_eq!(privacy.people, [alice, bob].into_iter().collect());
        assert_eq!((privacy.policy, privacy.owner), (PrivacyConfig::SharedAudience, ActorId::User(alice)));
    }
}
