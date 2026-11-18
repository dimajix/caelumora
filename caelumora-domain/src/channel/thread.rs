//! The threads of a channel (issue 557): the conversations within it, each named by its
//! `ThreadId`, the channel's own line among them.

use super::ChannelId;
use crate::actor::ActorId;
use crate::message::MessageId;
use serde::{Deserialize, Serialize};

define_id!(ThreadId);

impl ThreadId {
    /// The thread of `channel`'s own line, its id derived from the channel's: what every event
    /// made for the channel happens in, unless it was made for another thread.
    pub fn line(channel: ChannelId) -> Self {
        Self(channel.0)
    }
}

/// Where a thread hangs: a message of another thread of the same channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadOrigin {
    pub thread: ThreadId,
    pub message: MessageId,
}

/// How a thread was opened: where it hangs, and who opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    pub origin: ThreadOrigin,
    pub by: ActorId,
}

/// A thread of a channel, as the channel keeps it (`ChannelState::threads`): what it builds the
/// thread from as it starts, and whether it was closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadState {
    /// How it was opened; `None` for the channel's line, which hangs nowhere and is there from the
    /// channel's start.
    pub opening: Option<Opening>,
    /// Whether it was closed for good: nothing more is said in it. The line never is.
    pub closed: bool,
}

impl ThreadState {
    /// The state of a channel's line.
    pub const LINE: ThreadState = ThreadState { opening: None, closed: false };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue 557: a channel's line is named by the channel alone - the same for the same channel,
    /// another for another.
    #[test]
    fn the_line_of_a_channel_is_named_by_the_channel() {
        let (one, other) = (ChannelId::new(), ChannelId::new());
        assert_eq!(ThreadId::line(one), ThreadId::line(one), "the same channel, the same line");
        assert_ne!(ThreadId::line(one), ThreadId::line(other), "another channel, another line");
    }
}
