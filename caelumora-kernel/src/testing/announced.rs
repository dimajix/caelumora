//! `Announced`: an event publisher that records what is announced, in order - for the tests of
//! what a writer announces.

use caelumora_domain::confidentiality::Classification;
use caelumora_domain::event::{EventPublisher, GlobalEvent};
use caelumora_domain::security::SecurityAttributes;
use std::mem;
use std::sync::Mutex;

/// What was announced, in order, whoever it reaches.
#[derive(Debug, Default)]
pub(crate) struct Announced(Mutex<Vec<GlobalEvent>>);

impl Announced {
    /// What was announced since it was last taken, in order.
    pub(crate) fn taken(&self) -> Vec<GlobalEvent> {
        mem::take(&mut *self.0.lock().unwrap())
    }
}

impl EventPublisher for Announced {
    fn publish(&self, event: GlobalEvent, _attributes: SecurityAttributes) {
        self.0.lock().unwrap().push(event);
    }

    fn publish_classified(
        &self,
        event: GlobalEvent,
        attributes: SecurityAttributes,
        _classifications: &[Classification],
    ) {
        self.publish(event, attributes);
    }
}
