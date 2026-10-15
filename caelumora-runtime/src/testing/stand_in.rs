//! An actor that stands in a channel or before a moderator and does nothing
//! (issue 372): for the tests of what a channel or a moderator does with its
//! participants - whom it finds, wakes or cancels - where who they are does
//! not matter and no agent has to be stored. An agent the runtime builds
//! itself is a `ScriptedAgent`.

use crate::actor::Actor;
use crate::transport::{EventReceiver, EventSender};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::{ActorId, ActorState, Envelope};
use caelumora_model::value::name::Name;
use std::sync::atomic::{AtomicBool, Ordering};

/// An actor that does nothing: in the state it is put in until it is woken,
/// then idle. It keeps whether it was woken or cancelled.
#[derive(Debug)]
pub(crate) struct StandIn {
    id: ActorId,
    name: Name,
    state: ActorState,
    woken: AtomicBool,
    cancelled: AtomicBool,
}

impl StandIn {
    /// The actor `id`, called `name`, in `state`.
    ///
    /// # Panics
    /// If `name` is no valid name.
    pub(crate) fn new(id: ActorId, name: &str, state: ActorState) -> Self {
        Self { id, name: Name::new(name).expect("a name"), state, woken: AtomicBool::new(false), cancelled: AtomicBool::new(false) }
    }

    /// The actor `id` asleep, called `name`.
    pub(crate) fn sleeping(id: ActorId, name: &str) -> Self {
        Self::new(id, name, ActorState::Sleeping)
    }

    /// Whether it was woken (`Actor::wakeup`) since it was built or put back
    /// to sleep.
    pub(crate) fn woken(&self) -> bool {
        self.woken.load(Ordering::SeqCst)
    }

    /// Puts it back to sleep, as before it was woken.
    pub(crate) fn sleep(&self) {
        self.woken.store(false, Ordering::SeqCst);
    }

    /// Whether its turn was ended (`Actor::cancel`).
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Actor for StandIn {
    fn id(&self) -> ActorId {
        self.id
    }

    fn name(&self) -> Name {
        self.name.clone()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(&self, _rx: Box<dyn EventReceiver>, _tx: Box<dyn EventSender>) -> Result<()> {
        Ok(())
    }

    fn state(&self) -> ActorState {
        if self.woken() { ActorState::Idle } else { self.state }
    }

    fn wakeup(&self) {
        self.woken.store(true, Ordering::SeqCst);
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn stop(&self) {}
}
