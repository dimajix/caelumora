//! An answer of an agent under way in a channel, as the terminal keeps it from what the channel
//! shows (issue 569).

use caelumora_domain::actor::ActorId;
use caelumora_domain::work::TurnId;

/// An answer an agent has begun and not yet ended - said, given up -, as the web UI keeps its
/// answers under way.
#[derive(Debug)]
pub(crate) struct UnderWay {
    /// The agent that answers.
    pub(crate) agent: ActorId,
    /// The turn it answers in.
    pub(crate) turn: TurnId,
}
