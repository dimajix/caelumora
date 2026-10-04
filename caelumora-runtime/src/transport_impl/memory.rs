use crate::transport::{EventPredicate, EventReceiver, EventSender, TransportError, TransportResult};
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

    async fn recv_matching(&mut self, pred: EventPredicate) -> Option<Envelope> {
        if let Some(event) = self.buffer.iter().position(pred).and_then(|i| self.buffer.remove(i)) {
            self.uncommitted.push(event.clone());
            return Some(event);
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

    fn try_recv(&mut self) -> Option<Envelope> {
        let event = self.buffer.pop_front().or_else(|| self.rx.try_recv().ok())?;
        self.uncommitted.push(event.clone());
        Some(event)
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
    async fn try_recv_takes_what_was_put_aside_before_what_is_queued_and_never_waits() {
        let (tx, rx) = mpsc::channel(4);
        let mut receiver = InMemoryEventReceiver::new(rx);
        assert!(receiver.try_recv().is_none(), "nothing queued, nothing waited for");

        tx.try_send(envelope(ActorEvent::ResponseRequested)).expect("room");
        tx.try_send(envelope(ActorEvent::FullResponseGenerated(Vec::new()))).expect("room");
        tx.try_send(envelope(ActorEvent::ResponseStarted)).expect("room");
        // Puts the request aside while it takes what it waits for.
        receiver.recv_matching(|e| matches!(e.event, ActorEvent::FullResponseGenerated(_))).await.expect("queued");

        let kinds: Vec<_> = std::iter::from_fn(|| receiver.try_recv()).map(|e| e.event.kind()).collect();
        assert_eq!(kinds, ["ResponseRequested", "ResponseStarted"], "what was put aside first, then what is queued");
        assert_eq!(receiver.uncommitted.len(), 3, "uncommitted like what recv delivers");
        receiver.commit().await.expect("in memory");
        assert!(receiver.uncommitted.is_empty(), "committed with the next commit");

        drop(tx);
        assert!(receiver.try_recv().is_none(), "nothing either once the input is closed");
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
