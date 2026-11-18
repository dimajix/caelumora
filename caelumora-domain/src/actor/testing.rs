//! Contexts of events for the tests of this crate and of others, which write beside them only what
//! they differ in.

use super::{ActorId, EventContext};
use crate::channel::ChannelId;
use crate::session::SessionId;

/// The context of an event made for `channel` by nobody in particular, in a session of its own: in
/// the channel's line (issue 557), where a test records what happened by hand (issue 599) - what
/// it sets the thread, the sender and the rest of in. A test that knows its channel's session makes
/// it with `EventContext::new` instead.
pub fn context_in(channel: ChannelId) -> EventContext {
    EventContext::new(SessionId::new(), channel, ActorId::None, String::new())
}
