//! Channels and their states for the tests of this crate and of others, which write beside them
//! only what they differ in.

use super::{moderation_components, Channel, ChannelState};
use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::history::{Audience, HistoryEntry, Recorded};
use crate::security::SecurityAttributes;
use crate::session::{Session, SessionId};
use crate::value::name::Name;
use chrono::Utc;

/// A channel `name` of the session `session_id` as `Channel::build` makes one - broadcast to
/// its participants, of the default classification, without participants - but the system's,
/// and read by nobody else (`SecurityAttributes::systems()`).
#[expect(
    clippy::expect_used,
    reason = "test code: a name that is none fails the test that gives it"
)]
pub fn make_channel(session_id: SessionId, name: &str) -> Channel {
    let built = Channel::build(
        session_id,
        ActorId::System,
        Name::new(name).expect("a test's name of a channel is one"),
        moderation_components(None, Some("broadcast_policy")),
        Classification::CHANNEL_DEFAULT,
        None,
    );
    Channel { security_attributes: SecurityAttributes::systems(), ..built }
}

/// A channel of the session `session_id` that holds nothing but its ids: what an actor outside
/// a running session is in (the kernel's `ChannelRuntime::outside_a_session`) - neither
/// components nor the workspace of its session.
pub fn bare_channel(session_id: SessionId) -> Channel {
    Channel {
        components: Default::default(),
        uses_workspace: false,
        ..make_channel(session_id, "c")
    }
}

/// The channel `session` lists, as a document of its own (issue 70) and as `Channel::build`
/// makes one: "main", belonging to whom the session belongs to, read by its participants -
/// the session's participants. `session` lists at least one channel
/// (`session::testing::make_session`).
pub fn main_channel(session: &Session) -> Channel {
    let id = session.channels[0];
    // The id the session lists, and the attributes that go with it.
    Channel {
        id,
        security_attributes: Channel::attributes_of(id, session.security_attributes.owner_id),
        participants: session.participants.clone(),
        ..make_channel(session.id, "main")
    }
}

/// The state of `channel` - a channel a test stores as it is, its participants written into
/// its document - with their joins recorded in its history, in its owner's name, as if they
/// had founded it (issue 420): so that its history says who takes part, as it does of a
/// channel created by the one way in. What a test records in the history goes after them.
pub fn founded(channel: &Channel) -> ChannelState {
    let mut state = ChannelState::new(channel.id);
    let owner = channel.security_attributes.owner_id;
    for participant in &channel.participants {
        let actor = participant.actor_id();
        let joined = HistoryEntry::ParticipantJoined {
            participant: actor,
            name: actor.to_string(),
            by: owner,
            by_name: owner.to_string(),
            at: Utc::now(),
        };
        state.history.enter(Recorded::new(joined, owner, Audience::Everybody));
    }
    state
}
