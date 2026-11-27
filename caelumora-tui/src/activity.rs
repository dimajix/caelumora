//! What goes on in the session followed, as the screen shows it: the answers of the thread
//! followed, and every place the user can be (issues 572, 615).

use crate::under_way::UnderWay;

/// What goes on in the session followed: its name, the answers of the thread followed, and every
/// place the user can be - each channel followed, each thread of the channel talked in - with
/// where agents are at work or something waits for the user.
#[derive(Debug, Clone, Default)]
pub(crate) struct Activity {
    /// The session, by name.
    pub(crate) session: String,
    /// The answers under way in the thread followed of the channel talked in, with what they
    /// streamed.
    pub(crate) here: Vec<UnderWay>,
    /// The channels followed, in the order of the session.
    pub(crate) channels: Vec<Place>,
    /// The line and the open threads of the channel talked in, the line first; none where no
    /// channel is talked in.
    pub(crate) threads: Vec<Place>,
}

/// A place the user can be - a channel followed, a thread of the channel talked in - and what goes
/// on there. Where the user is, nothing is marked: the status line shows the answers of the thread
/// followed, and the channel talked in asks what waits there itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    /// Its name: the channel's; `line` for the line, the agent a thread was delegated to, else
    /// `thread`.
    pub(crate) name: String,
    /// Whether the user is there: the channel talked in, the thread followed.
    pub(crate) current: bool,
    /// Whether an agent is at work there.
    pub(crate) at_work: bool,
    /// Whether an approval or questions wait for the user there.
    pub(crate) waits: bool,
}

impl Place {
    /// Whether something is marked there: an agent at work, or something waiting for the user.
    pub(crate) fn is_marked(&self) -> bool {
        self.at_work || self.waits
    }
}
