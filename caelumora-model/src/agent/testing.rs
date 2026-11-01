//! Personalities to write beside, for the tests of this crate and of others; agents stored with a
//! personality of their own are `caelumora_store::testing::stored_agent`.

use super::{Personality, PersonalityId};
use crate::scope::Scope;
use crate::security::SecurityAttributes;
use crate::value::name::ScopedName;

/// A personality `name` of the global scope, the system's as a seed's is, and naming nothing
/// else: internal, without roles, description or introduction, with the default components,
/// reasoning visibility and hearing during a turn.
#[expect(
    clippy::expect_used,
    reason = "test code: a name that is none fails the test that gives it"
)]
pub fn make_personality(name: &str) -> Personality {
    Personality {
        id: PersonalityId::new(),
        name: ScopedName::new(name, Scope::GlobalScope)
            .expect("a test's name of a personality is one"),
        kind: Default::default(),
        roles: None,
        description: None,
        introduction: None,
        components: Default::default(),
        security_attributes: SecurityAttributes::systems(),
        reasoning_visibility: Default::default(),
        hears_during_turn: Default::default(),
        deleted: None,
        revision: Default::default(),
        purging: false,
    }
}
