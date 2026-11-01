//! Privacy between sessions (issue 134): a session as privacy sees it (`SessionPrivacy`), and the
//! policy it chose (`PrivacyConfig`); the policies themselves, whom a session lets read what was
//! said in it, are `caelumora-security`'s (issue 517).
//!
//! Confidentiality (`crate::confidentiality`) decides by how sensitive
//! something is what *may* be shared; privacy decides whether the
//! participants of a session *agreed*. Within a session, its participants
//! share with each other and only the classifications decide. Beyond it,
//! the source session's privacy policy has to permit it as well.
//!
//! Every session has exactly one policy, chosen by the [`PrivacyConfig`]
//! stored with it.

use crate::actor::ActorId;
use crate::security::Secured;
use crate::session::{Participant, Session, SessionId};
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

/// A session as privacy sees it: which one it is, its policy, its owner,
/// and its people.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPrivacy {
    /// Within one session its participants share with each other, and only
    /// the classifications decide; beyond it, the policy has to permit it.
    pub id: SessionId,
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
        let people = session
            .participants
            .iter()
            .filter_map(|p| match p {
                Participant::User(user) => Some(*user),
                _ => None,
            })
            .chain(session.owner())
            .collect();
        Self { id: session.id, policy: session.privacy, owner, people }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityAttributes;
    use crate::session::testing::bare_session;

    #[test]
    fn the_people_of_a_session_are_its_users_and_its_owner() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let session = Session {
            participants: vec![Participant::User(bob), Participant::Agent(Default::default())],
            privacy: PrivacyConfig::SharedAudience,
            security_attributes: SecurityAttributes::owned_by(ActorId::User(alice)),
            ..bare_session(SessionId::new())
        };
        let privacy = SessionPrivacy::of(&session);
        assert_eq!(privacy.people, [alice, bob].into_iter().collect());
        assert_eq!(
            (privacy.policy, privacy.owner),
            (PrivacyConfig::SharedAudience, ActorId::User(alice))
        );
    }

    #[test]
    fn a_session_is_private_unless_it_says_otherwise() {
        assert_eq!(PrivacyConfig::default(), PrivacyConfig::Private, "the default");
        let names: Vec<_> =
            [PrivacyConfig::Private, PrivacyConfig::SameOwner, PrivacyConfig::SharedAudience]
                .iter()
                .map(|p| serde_json::to_value(p).unwrap())
                .collect();
        assert_eq!(
            names,
            ["private", "same_owner", "shared_audience"],
            "as the API and the web UI name them"
        );
    }
}
