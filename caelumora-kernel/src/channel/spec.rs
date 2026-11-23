//! What a caller chooses of a new thread of a channel: `ThreadSpec`, taken by
//! `procedure::thread::Open` and `procedure::thread::open`, which take its founders in, and by
//! `ChannelLoop::open_thread`, which opens it at its origin with its circle.

use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{Circle, ThreadOrigin};

/// What a new thread should be (issues 557, 602), as it was asked: where it hangs, whether who
/// takes part in it may grow, and whom it is founded with.
#[derive(Debug, Clone)]
pub struct ThreadSpec {
    pub origin: ThreadOrigin,
    /// Whether who takes part in it may grow; open unless asked otherwise.
    pub circle: Circle,
    /// Whom it is founded with - whom its opener acts for, or their own agents -; nobody unless
    /// named.
    pub founders: Vec<ActorId>,
}

impl ThreadSpec {
    /// A thread at `origin`, its circle open, founded with nobody.
    pub fn new(origin: ThreadOrigin) -> Self {
        Self { origin, circle: Circle::Open, founders: Vec::new() }
    }
}
