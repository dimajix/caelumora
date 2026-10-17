use arc_swap::{ArcSwap, Guard};
use caelumora_model::agent::Personality;
use std::sync::Arc;

/// A personality as this instance holds it, shared by the actors of all its
/// agents in all sessions (issue 406), so that every part of the runtime
/// that asks about a running agent's personality - its actors, whether it
/// may run, what it is limited to - is answered alike, without reading the
/// store at every use.
///
/// It is held beside the agent's controller, not by it: an agent's
/// personality is fixed with the agent (`Agent::personality_id`), and a
/// different one could make it a different kind of agent with different
/// components, which only a new actor can be.
///
/// The personality is not changed through it: personalities are written to
/// the store directly. What is stored is taken in whenever the
/// `ControllerManager` is asked for the controller - as an agent of it is
/// resolved to take part, and by the `CommandReaper` - and from then on
/// holds for every running agent of it; only their components stay as
/// they were built.
#[derive(Debug)]
pub struct PersonalityController {
    personality: ArcSwap<Personality>,
}

impl PersonalityController {
    /// `personality`, as this instance holds it. Made by the
    /// `ControllerManager`, which keeps one per personality.
    pub fn new(personality: Personality) -> Self {
        Self { personality: ArcSwap::from_pointee(personality) }
    }

    /// The personality as it stands.
    pub fn load(&self) -> Guard<Arc<Personality>> {
        self.personality.load()
    }

    /// Takes over `personality`, as it was read from the store.
    pub(super) fn take_over(&self, personality: Personality) {
        self.personality.store(Arc::new(personality));
    }
}
