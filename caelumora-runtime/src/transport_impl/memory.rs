use crate::transport::{EventReceiver, EventSender, TransportError, TransportResult};
use async_trait::async_trait;
use caelumora_model::actor::Envelope;
use std::collections::VecDeque;
use tokio::sync::mpsc;

pub struct InMemoryEventReceiver {
    rx: mpsc::Receiver<Envelope>,
    buffer: VecDeque<Envelope>,
    uncommitted: Vec<Envelope>,
}

impl InMemoryEventReceiver {
    pub fn new(rx: mpsc::Receiver<Envelope>) -> Self {
        Self {
            rx,
            buffer: VecDeque::new(),
            uncommitted: Vec::new(),
        }
    }

    /// Insert events at the front of the buffer (used for crash-recovery pre-load).
    pub fn prepend(&mut self, events: Vec<Envelope>) {
        for event in events.into_iter().rev() {
            self.buffer.push_front(event);
        }
    }
}

#[async_trait]
impl EventReceiver for InMemoryEventReceiver {
    async fn recv(&mut self) -> Option<Envelope> {
        if let Some(event) = self.buffer.pop_front() {
            self.uncommitted.push(event.clone());
            return Some(event);
        }
        let event = self.rx.recv().await?;
        self.uncommitted.push(event.clone());
        Some(event)
    }

    async fn recv_matching(&mut self, predicate: fn(&Envelope) -> bool) -> Option<Envelope> {
        // async_trait concretizes fn(&Envelope) -> bool to fn(&'concrete Envelope) -> bool
        // (binding the reference lifetime to a specific outer lifetime), which prevents the
        // predicate from being called with references created inside the async block.
        // Transmuting to the true HRTB form is safe: fn pointer types have the same runtime
        // representation regardless of lifetime annotation, and all predicates in this codebase
        // are pure observers that never store the reference they inspect.
        let pred: for<'a> fn(&'a Envelope) -> bool =
            unsafe { std::mem::transmute(predicate) };

        let mut i = 0;
        while i < self.buffer.len() {
            if pred(&self.buffer[i]) {
                let event = self.buffer.remove(i).unwrap();
                self.uncommitted.push(event.clone());
                return Some(event);
            }
            i += 1;
        }
        loop {
            let event = self.rx.recv().await?;
            let is_match = pred(&event);
            if is_match {
                self.uncommitted.push(event.clone());
                return Some(event);
            } else {
                self.buffer.push_back(event);
            }
        }
    }

    fn drain_matching(&mut self, predicate: fn(&Envelope) -> bool) -> Vec<Envelope> {
        let mut matched = Vec::new();
        let mut remaining = VecDeque::new();
        while let Some(event) = self.buffer.pop_front() {
            if predicate(&event) {
                self.uncommitted.push(event.clone());
                matched.push(event);
            } else {
                remaining.push_back(event);
            }
        }
        self.buffer = remaining;
        matched
    }

    fn take_buffered(&mut self) -> Vec<Envelope> {
        std::mem::take(&mut self.buffer).into_iter().collect()
    }

    async fn commit(&mut self) -> TransportResult<()> {
        self.uncommitted.clear();
        Ok(())
    }
}

pub struct InMemoryEventSender {
    tx: mpsc::Sender<Envelope>,
}

impl InMemoryEventSender {
    pub fn new(tx: mpsc::Sender<Envelope>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl EventSender for InMemoryEventSender {
    async fn send(&self, event: Envelope) -> TransportResult<()> {
        self.tx
            .send(event)
            .await
            .map_err(|_| TransportError::Closed)
    }

    fn try_send(&self, event: Envelope) -> TransportResult<()> {
        self.tx.try_send(event).map_err(TransportError::from)
    }
}

/// Decorator that applies a filter predicate before forwarding to an inner `EventSender`.
/// An event the filter rejects is dropped and the send succeeds, on `send()` and
/// `try_send()` alike: filtering is what this sender is for, not a failure (issue 115).
/// Implements `EventSender`, so it can be composed with another `FilteringEventSender`.
pub struct FilteringEventSender {
    inner: Box<dyn EventSender>,
    filter: fn(&Envelope) -> bool,
}

impl FilteringEventSender {
    pub fn new(inner: Box<dyn EventSender>, filter: fn(&Envelope) -> bool) -> Self {
        Self { inner, filter }
    }
}

#[async_trait]
impl EventSender for FilteringEventSender {
    async fn send(&self, event: Envelope) -> TransportResult<()> {
        if (self.filter)(&event) {
            self.inner.send(event).await
        } else {
            Ok(())
        }
    }

    fn try_send(&self, event: Envelope) -> TransportResult<()> {
        if (self.filter)(&event) {
            self.inner.try_send(event)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{ActorEvent, EventContext};
    use caelumora_model::security::SecurityContext;

    fn envelope(event: ActorEvent) -> Envelope {
        Envelope::new(SecurityContext::default(), EventContext::default(), event)
    }

    #[tokio::test]
    async fn a_filtered_event_is_dropped_and_both_sends_succeed() {
        let (tx, mut rx) = mpsc::channel(4);
        let sender = FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            |e| matches!(e.event, ActorEvent::ResponseRequested),
        );

        sender.send(envelope(ActorEvent::ResponseStarted)).await.expect("filtered, not failed");
        sender.try_send(envelope(ActorEvent::ResponseStarted)).expect("filtered, not failed");
        sender.try_send(envelope(ActorEvent::ResponseRequested)).expect("passes the filter");

        assert!(matches!(rx.try_recv().map(|e| e.event), Ok(ActorEvent::ResponseRequested)), "only what passes arrives");
        assert!(rx.try_recv().is_err(), "nothing else");
    }
}
