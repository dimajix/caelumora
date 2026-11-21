//! A thread of a running channel (issue 557): what decides whom the events that happen in it go
//! to and who is asked to respond in it.

use crate::moderator::{Moderator, Routing};
use caelumora_api::actor::Actor;
use caelumora_api::component::ComponentImpl;
use caelumora_domain::actor::{ActorId, Envelope};
use caelumora_domain::channel::Opening;
use caelumora_security::SecurityContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A thread of a running channel (issue 557): the conversation of the events whose context names
/// it (`EventContext::thread_id`), with a moderator of its own, built for it - so that what the
/// moderator keeps of a conversation, and its policy of whose turn it is, are the thread's. The
/// channel holds one for each of its threads, its line among them; it admits an event, has the
/// event's thread decide its `Routing`, and records and delivers it. A thread closed is no longer
/// held: the channel's state tells it from one it never had (`ChannelState::threads`).
#[derive(Debug)]
pub(crate) struct Thread {
    moderator: ComponentImpl<dyn Moderator>,
    /// How it was opened; `None` for the line.
    opening: Option<Opening>,
    /// Set by a cancel, cleared as it is woken: while set, what is said in it is still delivered,
    /// but nobody is asked to respond to it (issue 557, as the channel was before). Held in memory
    /// alone: after a restart no thread is quiet.
    quiet: AtomicBool,
}

impl Thread {
    /// A thread opened as `opening` says - `None` for the line -, moderated by `moderator`, built
    /// for it.
    pub(crate) fn new(moderator: ComponentImpl<dyn Moderator>, opening: Option<Opening>) -> Self {
        Self { moderator, opening, quiet: AtomicBool::new(false) }
    }

    /// Whether nobody is asked to respond in it, after a cancel, until it is woken.
    pub(crate) fn is_quiet(&self) -> bool {
        self.quiet.load(Ordering::SeqCst)
    }

    /// Makes it quiet, or wakes it.
    pub(crate) fn set_quiet(&self, quiet: bool) {
        self.quiet.store(quiet, Ordering::SeqCst);
    }

    /// Where it was opened and by whom; `None` for the line, which nobody opened.
    pub(crate) fn opening(&self) -> Option<Opening> {
        self.opening
    }

    /// What becomes of `event`, which happens in this thread, admitted in `context`
    /// (`Moderator::route`): whom it is delivered to, whom to ask to respond, and what the
    /// moderator tells of its own.
    pub(crate) async fn route(&self, event: &Envelope, context: &SecurityContext) -> Routing {
        self.moderator.route(event, context).await
    }

    /// `actor` takes part in the thread: its moderator may deliver to it and ask it.
    pub(crate) fn add_actor(&self, actor: Arc<dyn Actor>) {
        self.moderator.add_actor(actor);
    }

    /// `actor` no longer takes part in the thread.
    pub(crate) fn remove_actor(&self, actor: ActorId) {
        self.moderator.remove_actor(actor);
    }
}
