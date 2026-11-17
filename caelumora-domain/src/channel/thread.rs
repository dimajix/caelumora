//! The threads of a channel (issue 557): the conversations within it, each named by its
//! `ThreadId`, the channel's own line among them.

use super::ChannelId;

define_id!(ThreadId);

impl ThreadId {
    /// The thread of `channel`'s own line, its id derived from the channel's: what every event
    /// made for the channel happens in, unless it was made for another thread.
    pub fn line(channel: ChannelId) -> Self {
        Self(channel.0)
    }
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
