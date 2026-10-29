//! An actor's inbox in its channel (issue 223): a bounded queue of the
//! envelopes the channel delivers live, and behind it the channel's history,
//! which the inbox reads from wherever the queue fell short. An overflow, a
//! restart and a join are one case: the inbox is behind, and reads on from
//! where its actor stands (`Heard`) until it has caught up.
//!
//! A live envelope carries the security context its sender set as it sent
//! it - what was true as the request was given. One read from the history
//! carries the sender's context as it is now (`Behind::recorded_for`): less
//! exact, and the fallback alone. Nothing important is lost, since
//! everything important is recorded; what is not recorded, and so may be
//! lost while the inbox is behind, is transient (`ActorEvent::is_transient`).

use crate::transport::{EventPick, EventPredicate, EventReceiver};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::{ActorId, Envelope, Pick};
use caelumora_model::chain::Position;
use caelumora_model::history::Heard;
use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::sync::Notify;
use tracing::{debug, warn};

/// How many envelopes an inbox's queue holds at most, and how many entries
/// of the history are read at once.
pub(crate) const CAPACITY: usize = 100;

/// The channel's history behind an inbox (issue 223): what an actor is
/// delivered of it where its queue fell short - read as the actor, built
/// into envelopes and stamped as the channel delivers them live. The
/// channel's (`ChannelLoop`); a test's in the tests.
#[async_trait]
pub(crate) trait Behind: Send + Sync {
    /// What `actor` is delivered of the history from the position `from`
    /// on, of at most `CAPACITY` entries read, and the position after the
    /// last entry read: where the next read begins - `from` once nothing is
    /// left to read.
    ///
    /// # Errors
    /// If the history cannot be read.
    async fn recorded_for(
        &self,
        actor: ActorId,
        from: Position,
    ) -> Result<(Vec<Envelope>, Position)>;
}

/// What both halves of an inbox share, under one lock.
struct Queue {
    envelopes: VecDeque<Envelope>,
    /// While the inbox is behind, the position after the last envelope it
    /// dropped: the history holds everything delivered to it from the first
    /// dropped on, the queue everything before.
    behind: Option<Position>,
    /// Up to where the receiving half read the history: what was delivered
    /// before it was read from there, and is not queued again.
    read_to: Position,
    /// The channel closed its half.
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    /// What was queued, dropped or closed - what the receiving half waits
    /// for.
    arrived: Notify,
    /// What the actor takes (`Actor::event_filter`).
    filter: fn(&Envelope) -> bool,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The channel's half of an actor's inbox: what it delivers to. Dropped, it
/// closes the inbox - the actor left, or the channel stops.
pub(crate) struct Inbox {
    actor: ActorId,
    shared: Arc<Shared>,
}

/// The actor's half of its inbox, what it receives from: the queue, and
/// once that fell short the history behind it, read through `channel`.
pub(crate) struct InboxReceiver {
    actor: ActorId,
    shared: Arc<Shared>,
    channel: Weak<dyn Behind>,
    heard: Heard,
    /// What was read of the history and not handed out yet, in order: a
    /// cache, a page at most, read again where need be.
    read: VecDeque<Envelope>,
}

/// What the receiving half found in the history.
enum Read {
    /// An envelope is there to be handed out (`InboxReceiver::read`).
    Ready,
    /// Nothing for the actor in what was read, but more to read.
    Again,
    /// Nothing more to read: the inbox has caught up, or waits for what is
    /// dropped next.
    Done,
    /// The channel is gone.
    Gone,
}

impl Inbox {
    /// The inbox of `actor`, which takes what passes `filter`, opened at
    /// `heard` in a history that ends at `end`: behind from the start if
    /// `heard` lies before it - the actor joins, or starts again - and read
    /// on through `channel` from there.
    pub(crate) fn open(
        actor: ActorId,
        filter: fn(&Envelope) -> bool,
        heard: Heard,
        end: Position,
        channel: Weak<dyn Behind>,
    ) -> (Inbox, InboxReceiver) {
        let behind = (heard.next() < end).then_some(end);
        let queue =
            Queue { envelopes: VecDeque::new(), behind, read_to: heard.next(), closed: false };
        let shared = Arc::new(Shared { queue: Mutex::new(queue), arrived: Notify::new(), filter });
        let receiver = InboxReceiver {
            actor,
            shared: Arc::clone(&shared),
            channel,
            heard,
            read: VecDeque::new(),
        };
        (Inbox { actor, shared }, receiver)
    }

    /// Delivers `envelope`, if the actor takes it: queued if there is room
    /// and the inbox is not behind. A recorded envelope there is no room
    /// for is dropped, and the inbox behind - the history has it -; one read
    /// from there already is dropped too. A transient one is dropped while
    /// the queue is full or the inbox behind, at debug level: it may be
    /// lost. Anything else is queued regardless: the history does not have
    /// it.
    pub(crate) fn deliver(&self, envelope: Envelope) {
        if !(self.shared.filter)(&envelope) {
            return;
        }
        {
            let mut queue = self.shared.queue();
            let full = queue.envelopes.len() >= CAPACITY;
            match envelope.context.position {
                Some(seq) if seq < queue.read_to => return,
                Some(seq) if full || queue.behind.is_some() => {
                    if queue.behind.is_none() {
                        debug!("The inbox of {} is full: what is delivered to it from position {} on is read from the history", self.actor, seq);
                    }
                    queue.behind =
                        Some(queue.behind.map_or(seq.next(), |behind| behind.max(seq.next())));
                }
                None if envelope.event.is_transient() && (full || queue.behind.is_some()) => {
                    debug!("Dropped {} for {}: its inbox is full", envelope.describe(), self.actor);
                    return;
                }
                _ => queue.envelopes.push_back(envelope),
            }
        }
        self.shared.arrived.notify_one();
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.shared.queue().closed = true;
        self.shared.arrived.notify_one();
    }
}

impl InboxReceiver {
    /// `envelope`, handed out in order.
    fn took(&mut self, envelope: Envelope) -> Envelope {
        if let Some(seq) = envelope.context.position {
            self.heard.took(seq);
        }
        envelope
    }

    /// `envelope`, handed out ahead of what comes before it.
    fn took_ahead(&mut self, envelope: Envelope) -> Envelope {
        if let Some(seq) = envelope.context.position {
            self.heard.took_ahead(seq);
        }
        envelope
    }

    /// Whether `envelope` is for the actor and was not handed out already.
    fn fresh(&self, envelope: &Envelope) -> bool {
        (self.shared.filter)(envelope)
            && !envelope.context.position.is_some_and(|seq| self.heard.has(seq))
    }

    /// Reads the history on until an envelope for the actor is there to be
    /// handed out, a page at a time, from where it stands - and once
    /// nothing is left to read and no more was dropped, the inbox is no
    /// longer behind. Cancel-safe: what was read is kept only once the read
    /// is done.
    async fn read(&mut self) -> Read {
        while self.read.front().is_some_and(|envelope| !self.fresh(envelope)) {
            self.read.pop_front();
        }
        if !self.read.is_empty() {
            return Read::Ready;
        }
        let Some(channel) = self.channel.upgrade() else { return Read::Gone };
        let from = self.shared.queue().read_to.max(self.heard.next());
        let (envelopes, to) = match channel.recorded_for(self.actor, from).await {
            Ok(read) => read,
            Err(e) => {
                // Read again once something arrives.
                warn!(
                    "Cannot read the history of the channel of {} from position {}: {:#}",
                    self.actor, from, e
                );
                return Read::Done;
            }
        };
        let fresh: Vec<Envelope> =
            envelopes.into_iter().filter(|envelope| self.fresh(envelope)).collect();
        self.read.extend(fresh);
        let mut queue = self.shared.queue();
        queue.read_to = queue.read_to.max(to);
        if !self.read.is_empty() {
            Read::Ready
        } else if to > from {
            Read::Again
        } else {
            if queue.behind.is_some_and(|behind| to >= behind) {
                queue.behind = None;
            }
            Read::Done
        }
    }

    /// The first envelope `pick` takes - from the queue, and once the inbox
    /// is behind from the history behind it: what was read of it, then the
    /// history itself -, looked at in order; what it passes over stays where
    /// it is, and nothing past what it stops at is taken. Waits for one if
    /// `wait`. Cancel-safe.
    async fn next(&mut self, wait: bool, pick: &EventPick<'_>) -> Option<Envelope> {
        // Searched already in this call: what arrives meanwhile is read
        // from there on.
        let mut searched: Option<Position> = None;
        loop {
            let (behind, passed) = {
                let mut queue = self.shared.queue();
                match Self::first(queue.envelopes.iter(), pick) {
                    Found::Taken(at) => {
                        let envelope = queue.envelopes.remove(at)?;
                        drop(queue);
                        return Some(if at == 0 {
                            self.took(envelope)
                        } else {
                            self.took_ahead(envelope)
                        });
                    }
                    Found::Stopped => return None,
                    Found::Passed => {}
                }
                if queue.closed {
                    return None;
                }
                (queue.behind.is_some(), !queue.envelopes.is_empty())
            };
            if behind {
                // What the queue still holds lies before what is read of the
                // history: read on only once it is taken.
                if !passed {
                    match self.read().await {
                        Read::Ready => {}
                        Read::Again => continue,
                        // Caught up: the queue again.
                        Read::Done if self.shared.queue().behind.is_none() => continue,
                        Read::Done => {}
                        Read::Gone => return None,
                    }
                }
                // Without what was handed out already, taken ahead.
                let read = mem::take(&mut self.read);
                self.read = read.into_iter().filter(|envelope| self.fresh(envelope)).collect();
                match Self::first(self.read.iter(), pick) {
                    Found::Taken(at) => {
                        let envelope = self.read.remove(at)?;
                        return Some(if !passed && at == 0 {
                            self.took(envelope)
                        } else {
                            self.took_ahead(envelope)
                        });
                    }
                    Found::Stopped => return None,
                    Found::Passed => {}
                }
                let from =
                    searched.unwrap_or_else(|| self.shared.queue().read_to.max(self.heard.next()));
                match self.search(from, pick).await {
                    Searched::Taken(envelope) => return Some(self.took_ahead(*envelope)),
                    Searched::Stopped => return None,
                    Searched::Passed(to) => searched = Some(to),
                    Searched::Gone => return None,
                }
            }
            if !wait {
                return None;
            }
            self.shared.arrived.notified().await;
        }
    }

    /// Where among `envelopes`, in order, the first that `pick` takes is,
    /// or whether it stops before one or passes over all of them.
    fn first<'e>(envelopes: impl Iterator<Item = &'e Envelope>, pick: &EventPick<'_>) -> Found {
        for (at, envelope) in envelopes.enumerate() {
            match pick(envelope) {
                Pick::Take => return Found::Taken(at),
                Pick::PassOver => {}
                Pick::Stop => return Found::Stopped,
            }
        }
        Found::Passed
    }

    /// Looks for the first envelope `pick` takes in the history from `from`
    /// on, without keeping what it passes over: that stays where it is, to
    /// be read again in order.
    async fn search(&self, from: Position, pick: &EventPick<'_>) -> Searched {
        let Some(channel) = self.channel.upgrade() else { return Searched::Gone };
        let mut from = from;
        loop {
            let (envelopes, to) = match channel.recorded_for(self.actor, from).await {
                Ok(read) => read,
                Err(e) => {
                    warn!(
                        "Cannot search the history of the channel of {} from position {}: {:#}",
                        self.actor, from, e
                    );
                    return Searched::Passed(from);
                }
            };
            let mut fresh: Vec<Envelope> =
                envelopes.into_iter().filter(|envelope| self.fresh(envelope)).collect();
            match Self::first(fresh.iter(), pick) {
                Found::Taken(at) => return Searched::Taken(Box::new(fresh.swap_remove(at))),
                Found::Stopped => return Searched::Stopped,
                Found::Passed => {}
            }
            if to <= from {
                return Searched::Passed(from);
            }
            from = to;
        }
    }
}

/// Where in what is looked at the envelope a pick takes is (`InboxReceiver::first`).
enum Found {
    /// At this index.
    Taken(usize),
    /// Nowhere: it stopped before one.
    Stopped,
    /// Nowhere: it passed over all of them.
    Passed,
}

/// What a search of the history found (`InboxReceiver::search`).
enum Searched {
    /// The envelope taken, boxed: the other variants are small.
    Taken(Box<Envelope>),
    /// Nothing: it stopped before one.
    Stopped,
    /// Nothing up to this position, where the next search goes on.
    Passed(Position),
    /// The channel is gone.
    Gone,
}

#[async_trait]
impl EventReceiver for InboxReceiver {
    async fn recv(&mut self) -> Option<Envelope> {
        self.next(true, &|_| Pick::Take).await
    }

    /// Searches the queue, then what was read of the history, then the
    /// history itself, without keeping what it passes over: that stays
    /// where it is, to be read again in order.
    async fn recv_matching(&mut self, predicate: EventPredicate) -> Option<Envelope> {
        self.next(true, &|envelope| if predicate(envelope) { Pick::Take } else { Pick::PassOver })
            .await
    }

    async fn try_recv(&mut self) -> Option<Envelope> {
        self.next(false, &|_| Pick::Take).await
    }

    async fn try_recv_if(&mut self, pick: &EventPick<'_>) -> Option<Envelope> {
        self.next(false, pick).await
    }

    fn heard(&self) -> Heard {
        self.heard.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{ActorEvent, EventContext, RequestReason};
    use caelumora_model::approval::ApprovalResponse;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::Message;
    use caelumora_model::question::QuestionId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::work::TurnId;
    use std::ops::Range;
    use std::time::Duration;
    use tokio::time;

    /// A history of envelopes, each at its index, as the actor is delivered
    /// them.
    #[derive(Default)]
    struct History(Mutex<Vec<Envelope>>);

    impl History {
        /// Records `event` and returns it as it is delivered live.
        fn record(&self, event: ActorEvent) -> Envelope {
            let mut all = self.0.lock().unwrap();
            let envelope = Envelope::new(
                SecurityContext::default(),
                EventContext {
                    position: Some(Position::new(all.len() as u64)),
                    ..EventContext::default()
                },
                event,
            );
            all.push(envelope.clone());
            envelope
        }

        fn end(&self) -> Position {
            Position::new(self.0.lock().unwrap().len() as u64)
        }
    }

    #[async_trait]
    impl Behind for History {
        async fn recorded_for(
            &self,
            _actor: ActorId,
            from: Position,
        ) -> Result<(Vec<Envelope>, Position)> {
            let all = self.0.lock().unwrap();
            let start = usize::try_from(from.get()).unwrap().min(all.len());
            let end = (start + CAPACITY).min(all.len());
            Ok((all[start..end].to_vec(), Position::new(end as u64)))
        }
    }

    fn said(n: usize) -> ActorEvent {
        ActorEvent::CompleteMessageGenerated(Message {
            content: Some(n.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        })
    }

    fn content(envelope: &Envelope) -> String {
        match &envelope.event {
            ActorEvent::CompleteMessageGenerated(message) => {
                message.content.clone().unwrap_or_default()
            }
            other => other.kind().to_string(),
        }
    }

    fn open(history: &Arc<History>, heard: Heard) -> (Inbox, InboxReceiver) {
        let behind: Arc<dyn Behind> = Arc::<History>::clone(history);
        Inbox::open(ActorId::None, |_| true, heard, history.end(), Arc::downgrade(&behind))
    }

    /// What `rx` hands out until nothing more is there.
    async fn all_of(rx: &mut InboxReceiver) -> Vec<String> {
        let mut all = Vec::new();
        while let Some(envelope) = rx.try_recv().await {
            all.push(content(&envelope));
        }
        all
    }

    fn numbers(range: Range<usize>) -> Vec<String> {
        range.map(|n| n.to_string()).collect()
    }

    /// Issue 223: what fits is queued and handed out in order; what does not
    /// is read from the history, in order, nothing twice and nothing
    /// missed, and once caught up the inbox queues again.
    #[tokio::test]
    async fn what_the_queue_cannot_hold_is_read_from_the_history() {
        let history = Arc::new(History::default());
        let (inbox, mut rx) = open(&history, Heard::default());
        for n in 0..CAPACITY + 150 {
            inbox.deliver(history.record(said(n)));
        }
        assert_eq!(inbox.shared.queue().envelopes.len(), CAPACITY, "no more than it holds");
        assert_eq!(all_of(&mut rx).await, numbers(0..CAPACITY + 150), "all of it, in order");
        assert_eq!(rx.heard(), Heard::at(history.end()));
        assert!(inbox.shared.queue().behind.is_none(), "caught up");
        inbox.deliver(history.record(said(1000)));
        assert_eq!(inbox.shared.queue().envelopes.len(), 1, "queued again");
        assert_eq!(all_of(&mut rx).await, ["1000"]);
    }

    /// Issue 223: an inbox opened before the history's end - a join, a
    /// restart - is behind from the start, and reads on from where its actor
    /// stands; what is delivered meanwhile comes after it.
    #[tokio::test]
    async fn an_inbox_opened_before_the_end_reads_on_from_there() {
        let history = Arc::new(History::default());
        for n in 0..10 {
            history.record(said(n));
        }
        let (inbox, mut rx) = open(&history, Heard::at(Position::new(4)));
        inbox.deliver(history.record(said(10)));
        assert_eq!(all_of(&mut rx).await, numbers(4..11));
    }

    /// Issue 223: while the inbox is behind, what is transient is dropped,
    /// but an answer to questions, which the history does not have, is
    /// queued.
    #[tokio::test]
    async fn while_behind_what_is_transient_is_lost_and_an_answer_kept() {
        let history = Arc::new(History::default());
        history.record(said(0));
        let (inbox, mut rx) = open(&history, Heard::default());
        let live =
            |event| Envelope::new(SecurityContext::default(), EventContext::default(), event);
        inbox.deliver(live(ActorEvent::ResponseStarted { turn_id: TurnId::new() }));
        inbox.deliver(live(ActorEvent::QuestionDeclined {
            id: QuestionId::new(),
            agent_id: Default::default(),
        }));
        assert_eq!(
            all_of(&mut rx).await,
            ["QuestionDeclined", "0"],
            "the answer, queued, comes first"
        );
    }

    /// Issue 223: a search takes what it looks for from the queue or the
    /// history ahead of the rest, which stays where it is and comes after,
    /// in order, without what was taken.
    #[tokio::test]
    async fn a_search_takes_ahead_and_leaves_the_rest_in_order() {
        let history = Arc::new(History::default());
        let (inbox, mut rx) = open(&history, Heard::default());
        let response = ApprovalResponse::new(Default::default(), Default::default());
        for n in 0..CAPACITY + 50 {
            let event = if n == CAPACITY + 20 {
                ActorEvent::ApprovalGranted(response.clone())
            } else {
                said(n)
            };
            inbox.deliver(history.record(event));
        }
        let found = rx
            .recv_matching(|e| matches!(e.event, ActorEvent::ApprovalGranted(_)))
            .await
            .expect("in the history");
        assert_eq!(found.context.position, Some(Position::new((CAPACITY + 20) as u64)));
        let rest = all_of(&mut rx).await;
        assert_eq!(rest.len(), CAPACITY + 49, "the rest");
        assert!(!rest.contains(&"ApprovalGranted".to_string()), "without what was taken");
        assert_eq!(rx.heard(), Heard::at(history.end()));

        let waiting = tokio::spawn(async move {
            rx.recv_matching(|e| matches!(e.event, ActorEvent::ApprovalDenied(_)))
                .await
                .map(|e| e.event.kind())
        });
        time::sleep(Duration::from_millis(20)).await;
        inbox.deliver(history.record(said(1)));
        inbox.deliver(history.record(ActorEvent::ApprovalDenied(response)));
        assert_eq!(
            time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap(),
            Some("ApprovalDenied"),
            "what arrives meanwhile"
        );
    }

    /// Issue 431: the next envelope is taken only if it may be, and stays
    /// the next otherwise - in the queue as in the history.
    #[tokio::test]
    async fn the_next_is_taken_only_if_it_may_be() {
        let history = Arc::new(History::default());
        history.record(said(0));
        history.record(ActorEvent::ResponseRequested { reason: RequestReason::Mentioned });
        let (_inbox, mut rx) = open(&history, Heard::default());
        let heard = |e: &Envelope| {
            if matches!(e.event, ActorEvent::CompleteMessageGenerated(_)) {
                Pick::Take
            } else {
                Pick::Stop
            }
        };
        assert!(rx.try_recv_if(&heard).await.is_some());
        assert!(rx.try_recv_if(&heard).await.is_none(), "a request waits");
        assert_eq!(all_of(&mut rx).await, ["ResponseRequested"], "and is still the next");
    }

    /// Takes a request to respond past what was said, and stops at
    /// anything else.
    fn requests_past_what_was_said(envelope: &Envelope) -> Pick {
        match envelope.event {
            ActorEvent::ResponseRequested { .. } => Pick::Take,
            ActorEvent::CompleteMessageGenerated(_) => Pick::PassOver,
            _ => Pick::Stop,
        }
    }

    fn command() -> ActorEvent {
        ActorEvent::CommandRequested {
            command: "mark".to_string(),
            args: String::new(),
            agent: ActorId::None,
        }
    }

    /// Issue 435: what is passed over stays where it is, in the queue as in
    /// the history behind it; what is taken past it is taken ahead, and
    /// nothing past a stop is taken.
    #[tokio::test]
    async fn what_is_passed_over_stays_where_it_is_and_nothing_past_a_stop_is_taken() {
        let history = Arc::new(History::default());
        let (inbox, mut rx) = open(&history, Heard::default());
        let (queued, read, stop) = (30, CAPACITY + 20, CAPACITY + 40);
        for n in 0..CAPACITY + 50 {
            let event = match n {
                n if n == queued || n == read => {
                    ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }
                }
                n if n == stop => command(),
                n => said(n),
            };
            inbox.deliver(history.record(event));
        }
        let position = |envelope: Option<Envelope>| {
            envelope.and_then(|e| e.context.position).map(|seq| seq.get() as usize)
        };
        assert_eq!(
            position(rx.try_recv_if(&requests_past_what_was_said).await),
            Some(queued),
            "from the queue"
        );
        assert_eq!(
            position(rx.try_recv_if(&requests_past_what_was_said).await),
            Some(read),
            "from the history behind it"
        );
        assert!(rx.try_recv_if(&requests_past_what_was_said).await.is_none(), "never past a stop");
        assert!(
            rx.heard().has(Position::new(read as u64)) && !rx.heard().has(Position::START),
            "taken ahead"
        );

        let rest = all_of(&mut rx).await;
        let mut expected: Vec<String> = (0..CAPACITY + 50)
            .filter(|n| *n != queued && *n != read)
            .map(|n| n.to_string())
            .collect();
        expected[stop - 2] = "CommandRequested".to_string();
        assert_eq!(rest, expected, "the rest, in order, once");
        assert_eq!(rx.heard(), Heard::at(history.end()));
    }

    /// Issue 435: what was read of the history is passed over as the queue
    /// is.
    #[tokio::test]
    async fn what_was_read_of_the_history_is_passed_over_as_the_queue_is() {
        let history = Arc::new(History::default());
        for event in [
            said(0),
            said(1),
            ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
            command(),
            ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
        ] {
            history.record(event);
        }
        let (_inbox, mut rx) = open(&history, Heard::default());
        let taken = rx.try_recv_if(&requests_past_what_was_said).await.expect("past what was said");
        assert_eq!(taken.context.position, Some(Position::new(2)));
        assert!(rx.try_recv_if(&requests_past_what_was_said).await.is_none(), "never past a stop");
        assert_eq!(all_of(&mut rx).await, ["0", "1", "CommandRequested", "ResponseRequested"]);
    }

    /// Issue 223: what the channel delivers of what was read from the
    /// history already is not handed out twice; a closed inbox ends once
    /// its queue is empty.
    #[tokio::test]
    async fn nothing_is_handed_out_twice_and_a_closed_inbox_ends() {
        let history = Arc::new(History::default());
        let (inbox, mut rx) = open(&history, Heard::default());
        for n in 0..CAPACITY + 1 {
            inbox.deliver(history.record(said(n)));
        }
        // Recorded, read from the history before the channel delivers it.
        let late = history.record(said(500));
        assert_eq!(all_of(&mut rx).await.len(), CAPACITY + 2);
        inbox.deliver(late);
        assert!(all_of(&mut rx).await.is_empty(), "read already");
        inbox.deliver(history.record(said(501)));
        drop(inbox);
        assert!(rx.recv().await.is_some(), "what was queued still");
        assert!(rx.recv().await.is_none(), "then nothing");
    }
}
