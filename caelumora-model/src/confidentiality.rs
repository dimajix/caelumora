//! Confidentiality levels (issue 61): how far a piece of information may
//! travel.
//!
//! A level is a reach. As the level of a channel it says how far the
//! channel's contents may travel; as the label of a part of a message, how
//! far that part may. What may be read or shown where depends on these
//! levels and on the channels involved alone - not on who takes part in a
//! channel at the moment, so that a decision stays the same when someone
//! joins or leaves. The `SecurityManager` makes the decisions; the rules of
//! the default policy are here. Compartments beside the levels are issue 64.

use crate::channel::ChannelId;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// How far information may travel, from `Public` (anywhere) to `Private`
/// (never leaves the actor). Ordered: a higher level is more confidential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidentiality {
    /// Anywhere, beyond the session too.
    Public,
    /// The channels of the same session.
    Session,
    /// Only the channel it belongs to.
    Channel,
    /// Only the actor it belongs to - never published to a channel; its
    /// owner sees it where they may see what the actor saw.
    Private,
}

impl Confidentiality {
    /// The level of a channel whose creator or template names none.
    pub const CHANNEL_DEFAULT: Confidentiality = Confidentiality::Session;

    /// Whether a channel may have this level: `Private` is the actor's, not
    /// a channel's.
    pub fn is_channel_level(&self) -> bool {
        *self != Confidentiality::Private
    }
}

impl fmt::Display for Confidentiality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Confidentiality::Public => "public",
            Confidentiality::Session => "session",
            Confidentiality::Channel => "channel",
            Confidentiality::Private => "private",
        })
    }
}

/// A channel as the confidentiality rules see it: where it is, and the level
/// of its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelClassification {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub level: Confidentiality,
}

impl ChannelClassification {
    /// Whether `reader` lies within the reach of this channel's contents.
    fn reaches(&self, reader: &ChannelClassification) -> bool {
        match self.level {
            Confidentiality::Public => true,
            Confidentiality::Session => self.session_id == reader.session_id,
            Confidentiality::Channel | Confidentiality::Private => self.channel_id == reader.channel_id,
        }
    }

    /// Whether the notes and history of this channel may be read from
    /// `reader` (rule 2 of issue 61): `reader` must lie within this
    /// channel's reach, and its own level must be at least this channel's,
    /// because an agent passes on what it reads to `reader`. A channel may
    /// always read itself.
    pub fn readable_from(&self, reader: &ChannelClassification) -> bool {
        self.channel_id == reader.channel_id || (self.reaches(reader) && self.level <= reader.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Confidentiality::*;

    fn channel(session: SessionId, level: Confidentiality) -> ChannelClassification {
        ChannelClassification { session_id: session, channel_id: ChannelId::new(), level }
    }

    #[test]
    fn the_levels_are_ordered_by_reach() {
        assert!(Public < Session && Session < Channel && Channel < Private);
        assert!(!Private.is_channel_level());
        assert_eq!(serde_json::to_value(Session).unwrap(), "session");
    }

    #[test]
    fn a_channel_is_read_within_its_reach_by_channels_at_least_as_confidential() {
        let (here, elsewhere) = (SessionId::new(), SessionId::new());
        let reader = |level| channel(here, level);

        // What may be read from a session-level channel, by source level.
        let session_reader = reader(Session);
        assert!(channel(here, Public).readable_from(&session_reader));
        assert!(channel(here, Session).readable_from(&session_reader));
        assert!(!channel(here, Channel).readable_from(&session_reader), "stays in its channel");
        assert!(!channel(elsewhere, Session).readable_from(&session_reader), "another session");
        assert!(channel(elsewhere, Public).readable_from(&session_reader), "public reaches everywhere");

        // A reader passes on what it reads: a public channel reads only
        // public contents, a channel-level one everything within reach.
        assert!(!channel(here, Session).readable_from(&reader(Public)));
        assert!(channel(here, Session).readable_from(&reader(Channel)));
        assert!(!channel(here, Channel).readable_from(&reader(Channel)), "not even by another confidential channel");
    }

    #[test]
    fn a_channel_always_reads_itself() {
        let own = channel(SessionId::new(), Channel);
        assert!(own.readable_from(&own));
    }
}
