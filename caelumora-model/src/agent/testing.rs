//! Personalities to write beside, and agents stored with a personality of their own, for the tests
//! of this crate and of others.

use super::{Agent, Personality, PersonalityId};
use crate::actor::ActorId;
use crate::scope::Scope;
use crate::security::{SecurityAttributes, SecurityContext};
use crate::store::EntityStores;
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

/// An agent `name` of the global scope, of `owner`, and its `personality`, both stored in `stores`
/// by the system.
#[expect(
    clippy::expect_used,
    reason = "test code: a name that is none, or a store that fails, fails the test"
)]
pub async fn stored_agent(
    stores: &EntityStores,
    name: &str,
    mut personality: Personality,
    owner: ActorId,
) -> Agent {
    let system = SecurityContext::system();
    stores.personalities.create(&system, &mut personality).await.expect("a personality stored");
    let mut agent = Agent::new(
        ScopedName::new(name, Scope::GlobalScope).expect("a test's name of an agent is one"),
        personality.id,
        owner,
    );
    stores.agents.create(&system, &mut agent).await.expect("an agent stored");
    agent
}
