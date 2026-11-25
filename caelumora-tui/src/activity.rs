//! What the agents of the channels followed are doing, as the screen shows it (issue 572).

use crate::under_way::UnderWay;
use caelumora_domain::value::name::Name;

/// What is at work in the channels followed: the answers of the thread followed, and where else
/// agents are at work or something waits for the user.
#[derive(Debug, Clone, Default)]
pub(crate) struct Activity {
    /// The answers under way in the thread followed of the channel talked in, with what they
    /// streamed.
    pub(crate) here: Vec<UnderWay>,
    /// The other channels followed where agents are at work or something waits for the user, in
    /// the order of the channels - the one talked in too, for its other threads.
    pub(crate) elsewhere: Vec<Elsewhere>,
}

/// A channel followed where agents are at work outside the thread followed, or something waits
/// for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Elsewhere {
    /// The channel, by name.
    pub(crate) channel: Name,
    /// The agents at work there outside the thread followed, by name, each once - also where it
    /// answers in several threads -, in the order they began.
    pub(crate) at_work: Vec<String>,
    /// Whether an approval or questions wait for the user there - never for the channel talked
    /// in, which asks them itself.
    pub(crate) waits: bool,
}
