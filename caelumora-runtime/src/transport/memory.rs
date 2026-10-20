use crate::transport::{EventPick, EventPredicate, EventReceiver, EventSender, TransportError, TransportResult};
use async_trait::async_trait;
use caelumora_model::actor::{ActorId, Envelope, Pick};
use caelumora_model::history::Heard;
use std::collections::VecDeque;
use tokio::sync::mpsc;

/// A queue an actor receives from, without a history behind it: for tests
/// of an actor, and for whatever runs an actor outside of a channel. What
/// `recv_matching` and `try_recv_if` pass over it puts aside, to be taken
/// first.
pub struct InMemoryEventReceiver {
    rx: mpsc::Receiver<Envelope>,
    buffer: VecDeque<Envelope>,
    heard: Heard,
}

impl InMemoryEventReceiver {
    pub fn new(rx: mpsc::Receiver<Envelope>) -> Self {
        Self { rx, buffer: VecDeque::new(), heard: Heard::default() }
    }

    /// `event`, taken in order.
    fn took(&mut self, event: Envelope) -> Envelope {
        if let Some(seq) = event.context.position {
            self.heard.took(seq);
        }
        event
    }
}

#[async_trait]
impl EventReceiver for InMemoryEventReceiver {
    async fn recv(&mut self) -> Option<Envelope> {
        let event = match self.buffer.pop_front() {
            Some(event) => event,
            None => self.rx.recv().await?,
        };
        Some(self.took(event))
    }

    async fn recv_matching(&mut self, pred: EventPredicate) -> Option<Envelope> {
        let found = match self.buffer.iter().position(pred).and_then(|i| self.buffer.remove(i)) {
            Some(event) => event,
            None => loop {
                let event = self.rx.recv().await?;
                if pred(&event) {
                    break event;
                }
                self.buffer.push_back(event);
            },
        };
        // Ahead of what it put aside, if anything is; in order otherwise.
        match (self.buffer.is_empty(), found.context.position) {
            (false, Some(seq)) => self.heard.took_ahead(seq),
            (true, Some(seq)) => self.heard.took(seq),
            (_, None) => {}
        }
        Some(found)
    }

    async fn try_recv(&mut self) -> Option<Envelope> {
        let event = self.buffer.pop_front().or_else(|| self.rx.try_recv().ok())?;
        Some(self.took(event))
    }

    async fn try_recv_if(&mut self, pick: &EventPick<'_>) -> Option<Envelope> {
        // What was put aside comes first; what is queued is looked at once
        // that is passed over, and waits in the buffer then.
        let mut at = 0;
        loop {
            if at == self.buffer.len() {
                let event = self.rx.try_recv().ok()?;
                self.buffer.push_back(event);
            }
            match pick(&self.buffer[at]) {
                Pick::Take => break,
                Pick::PassOver => at += 1,
                Pick::Stop => return None,
            }
        }
        let event = self.buffer.remove(at)?;
        match (at, event.context.position) {
            (0, _) => Some(self.took(event)),
            (_, Some(seq)) => {
                self.heard.took_ahead(seq);
                Some(event)
            }
            (_, None) => Some(event),
        }
    }

    fn heard(&self) -> Heard {
        self.heard.clone()
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
}

/// The sender an actor of a channel is given (issue 228): it passes on
/// only what speaks as the actor it is for (`speaks_as`) and refuses
/// anything else, so that the channel need not take the sender an
/// envelope names on trust. A refusal is a bug of the actor, and logged as
/// an error.
pub struct OriginEventSender {
    origin: ActorId,
    inner: Box<dyn EventSender>,
}

impl OriginEventSender {
    pub fn new(origin: ActorId, inner: Box<dyn EventSender>) -> Self {
        Self { origin, inner }
    }

    /// Whether `event` speaks as the actor: it names the actor as its
    /// sender and as the caller of its security context, and so does every
    /// message it carries. Whose rights it has, the context's principal,
    /// and who asked it, the initiator, may be someone else: an agent acts
    /// with its owner's rights for whoever asked it (issue 255).
    fn speaks_as(&self, event: &Envelope) -> bool {
        event.context.sender_id == self.origin
            && event.security.caller_id == self.origin
            && event.event.messages().iter().all(|m| m.sender_id == self.origin)
    }

    fn admits(&self, event: &Envelope) -> TransportResult<()> {
        if self.speaks_as(event) {
            return Ok(());
        }
        tracing::error!("Refused {}: sent by {} in the name of another", event.describe(), self.origin);
        Err(TransportError::Refused { origin: self.origin })
    }
}

#[async_trait]
impl EventSender for OriginEventSender {
    async fn send(&self, event: Envelope) -> TransportResult<()> {
        self.admits(&event)?;
        self.inner.send(event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{ActorEvent, EventContext, RequestReason};
    use caelumora_model::chain::Position;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::work::TurnId;

    fn envelope(event: ActorEvent) -> Envelope {
        Envelope::new(SecurityContext::default(), EventContext::default(), event)
    }

    /// `event`, delivered as the entry at the position `seq`.
    fn at(seq: u64, event: ActorEvent) -> Envelope {
        Envelope::new(SecurityContext::default(), EventContext { position: Some(Position::new(seq)), ..EventContext::default() }, event)
    }

    #[tokio::test]
    async fn try_recv_takes_what_was_put_aside_before_what_is_queued_and_never_waits() {
        let (tx, rx) = mpsc::channel(4);
        let mut receiver = InMemoryEventReceiver::new(rx);
        assert!(receiver.try_recv().await.is_none(), "nothing queued, nothing waited for");

        tx.try_send(at(0, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned })).expect("room");
        tx.try_send(at(1, ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: Vec::new() })).expect("room");
        tx.try_send(envelope(ActorEvent::ResponseStarted { turn_id: TurnId::new() })).expect("room");
        // Puts the request aside while it takes what it waits for.
        receiver.recv_matching(|e| matches!(e.event, ActorEvent::TurnFinished { .. })).await.expect("queued");
        assert!(receiver.heard().has(Position::new(1)) && receiver.heard().next() == Position::START, "taken ahead of what it put aside");

        let mut kinds = Vec::new();
        while let Some(event) = receiver.try_recv().await {
            kinds.push(event.event.kind());
        }
        assert_eq!(kinds, ["ResponseRequested", "ResponseStarted"], "what was put aside first, then what is queued");
        assert_eq!(receiver.heard(), Heard::at(Position::new(2)), "everything up to the last taken");

        drop(tx);
        assert!(receiver.try_recv().await.is_none(), "nothing either once the input is closed");
    }

    #[tokio::test]
    async fn try_recv_if_takes_the_next_event_only_if_it_fulfils_the_predicate_and_leaves_it_otherwise() {
        let (tx, rx) = mpsc::channel(4);
        let mut receiver = InMemoryEventReceiver::new(rx);
        let requested = |e: &Envelope| if matches!(e.event, ActorEvent::ResponseRequested { .. }) { Pick::Take } else { Pick::Stop };
        assert!(receiver.try_recv_if(&requested).await.is_none(), "nothing queued, nothing waited for");

        tx.try_send(envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned })).expect("room");
        tx.try_send(envelope(ActorEvent::ResponseStarted { turn_id: TurnId::new() })).expect("room");
        tx.try_send(envelope(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned })).expect("room");

        assert!(receiver.try_recv_if(&requested).await.is_some(), "the next event fulfils it");
        assert!(receiver.try_recv_if(&requested).await.is_none(), "the next one does not, and nothing behind it is looked at");
        assert!(receiver.try_recv_if(&requested).await.is_none(), "and stays the next one");

        let mut kinds = Vec::new();
        while let Some(event) = receiver.try_recv().await {
            kinds.push(event.event.kind());
        }
        assert_eq!(kinds, ["ResponseStarted", "ResponseRequested"], "what it left, in order");
    }

    /// Issue 435: what is passed over stays where it is, what is taken
    /// past it is taken ahead, and nothing past a stop is taken.
    #[tokio::test]
    async fn try_recv_if_takes_past_what_it_passes_over_and_never_past_a_stop() {
        let (tx, rx) = mpsc::channel(8);
        let mut receiver = InMemoryEventReceiver::new(rx);
        let requests_past_starts = |e: &Envelope| match e.event {
            ActorEvent::ResponseRequested { .. } => Pick::Take,
            ActorEvent::ResponseStarted { .. } => Pick::PassOver,
            _ => Pick::Stop,
        };
        let started = || ActorEvent::ResponseStarted { turn_id: TurnId::new() };
        for (seq, event) in [(0, started()), (1, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }), (2, started()), (3, ActorEvent::WakeupRequested), (4, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned })] {
            tx.try_send(at(seq, event)).expect("room");
        }

        let taken = receiver.try_recv_if(&requests_past_starts).await.expect("past what it passes over");
        assert_eq!(taken.context.position, Some(Position::new(1)));
        assert!(receiver.try_recv_if(&requests_past_starts).await.is_none(), "never past a stop");
        assert!(receiver.heard().has(Position::new(1)) && !receiver.heard().has(Position::new(0)), "taken ahead of what it passed over");

        let mut left = Vec::new();
        while let Some(event) = receiver.try_recv().await {
            left.push(event.context.position.map(|seq| seq.get()));
        }
        assert_eq!(left, [Some(0), Some(2), Some(3), Some(4)], "the rest, in order");
        assert_eq!(receiver.heard(), Heard::at(Position::new(5)));
    }

    #[tokio::test]
    async fn an_actor_s_sender_passes_on_only_what_speaks_as_the_actor() {
        use caelumora_model::agent::AgentId;
        let (tx, mut rx) = mpsc::channel(4);
        let agent = ActorId::Agent(AgentId::new());
        let sender = OriginEventSender::new(agent, Box::new(InMemoryEventSender::new(tx)));
        let as_actor = |sender_id: ActorId, caller: ActorId| Envelope::new(
            SecurityContext::default().next(caller, ActorId::None, ActorId::None),
            EventContext { sender_id, ..EventContext::default() },
            ActorEvent::ResponseStarted { turn_id: TurnId::new() },
        );
        let someone = ActorId::System;

        assert!(matches!(sender.send(as_actor(someone, agent)).await, Err(TransportError::Refused { origin }) if origin == agent), "another's envelope");
        assert!(matches!(sender.send(as_actor(agent, someone)).await, Err(TransportError::Refused { .. })), "another's context");
        sender.send(as_actor(agent, agent)).await.expect("its own");
        assert_eq!(rx.try_recv().map(|e| e.context.sender_id).ok(), Some(agent), "only its own is passed on");
        assert!(rx.try_recv().is_err());
    }

}
