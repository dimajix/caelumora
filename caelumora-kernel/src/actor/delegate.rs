//! The delegate of a thread (issue 557): the kernel's internal actor that carries what is
//! delegated into a thread - each brief given its recipient to read, one at a time - and brings
//! what came of it back to its giver, given to read where the thread was opened.

use anyhow::Result;
use async_trait::async_trait;
use caelumora_api::actor::{Actor, Answering};
use caelumora_api::transport::{EventReceiver, EventSender};
use caelumora_domain::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_domain::channel::{Brief, ChannelId, Delegation, ThreadId};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::history::Heard;
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::prompt_catalogue::{context, PromptCatalogue};
use caelumora_domain::session::SessionId;
use caelumora_domain::value::name::Name;
use caelumora_domain::work::TurnId;
use serde::Serialize;
use std::collections::HashMap;
use std::sync;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// The delegate of a thread (issue 557): built by the channel from what its state holds of the
/// thread's delegation, and kept in step with it as the channel changes it (`sync`) - the channel
/// alone moves a brief on, as it records or refuses what the delegate gives. It gives the open
/// brief to the recipient, in the thread, in the delegating chain's name; takes the recipient's
/// first turn that ends after the brief was given as its answer; and gives what came of it to the
/// giver where the thread was opened: the answer, that the turn ended without one, that it was
/// given up and why, that the recipient could not be asked - the brief refused -, or that it left.
/// It hears what goes to the users of its thread (`Recipients::reaches`): after a restart it reads
/// on from where its open brief was given (`heard`), so that an answer recorded meanwhile is not
/// lost.
#[derive(Debug)]
pub(crate) struct DelegateActor {
    thread: ThreadId,
    /// Where the thread was opened: where what came of a brief is given back.
    origin: ThreadId,
    session_id: SessionId,
    channel_id: ChannelId,
    /// What the recipient is called, which what is given back of it carries.
    recipient_name: String,
    prompts: Arc<PromptCatalogue>,
    /// The delegation as the channel's state holds it (`sync`).
    delegation: sync::Mutex<Delegation>,
    /// Woken as the delegation changes.
    changed: Notify,
    /// Cancelled by `stop` and `leave`: its run ends.
    ended: CancellationToken,
}

/// Where a delegate stands in its run: what it gave and waits for the channel to follow.
#[derive(Debug, Default)]
struct Carrying {
    /// The brief it gave last, until the channel records it given.
    briefed: Option<MessageId>,
    /// The brief it gave back what came of, until the channel takes the brief as answered.
    answered: Option<MessageId>,
    /// What the recipient said in each of its turns not ended yet: the answer, once the turn that
    /// answers the brief ends.
    said: HashMap<TurnId, Vec<Message>>,
    /// Whether the recipient left the thread: what is open is given back as such.
    gone: bool,
}

impl DelegateActor {
    /// The delegate of `thread` of the channel `channel_id` of session `session_id`, opened at
    /// `origin`, carrying `delegation` to its recipient, called `recipient_name`, telling what came
    /// of a brief by `prompts`.
    pub(crate) fn new(
        thread: ThreadId,
        origin: ThreadId,
        session_id: SessionId,
        channel_id: ChannelId,
        delegation: Delegation,
        recipient_name: String,
        prompts: Arc<PromptCatalogue>,
    ) -> Self {
        Self {
            thread,
            origin,
            session_id,
            channel_id,
            recipient_name,
            prompts,
            delegation: sync::Mutex::new(delegation),
            changed: Notify::new(),
            ended: CancellationToken::new(),
        }
    }

    /// Takes `delegation` as the channel's state holds it now: briefs delegated, given, answered
    /// or dropped since.
    pub(crate) fn sync(&self, delegation: Delegation) {
        *self.delegation() = delegation;
        self.changed.notify_one();
    }

    fn delegation(&self) -> sync::MutexGuard<'_, Delegation> {
        self.delegation.lock().unwrap_or_else(sync::PoisonError::into_inner)
    }

    /// The open brief, if one is.
    fn open(&self) -> Option<Brief> {
        self.delegation().briefs.front().cloned()
    }

    /// The context it gives in, in `thread`: its own, the delegating chain's initiator as the
    /// initiator - both directions answer the chain that delegated.
    fn context(&self, thread: ThreadId) -> EventContext {
        let id = ActorId::Delegate(self.thread);
        let own = Answering::own(self.session_id, self.channel_id, thread, id, self.name());
        EventContext { initiator_id: self.delegation().initiator, ..own.event_context }
    }

    /// Does what the open brief waits for of it: gives it to the recipient, once, in the thread -
    /// or, if the recipient left, gives back that it did.
    async fn carry(&self, carrying: &mut Carrying, tx: &dyn EventSender) {
        let Some(open) = self.open() else { return };
        if carrying.answered == Some(open.message.id) {
            return;
        }
        if carrying.gone {
            let text = self.told("kernel.delegation_left", context! {});
            self.give_back(carrying, &open, text, None, tx).await;
            return;
        }
        if open.given_at.is_some() || carrying.briefed == Some(open.message.id) {
            return;
        }
        let recipient = self.delegation().recipient;
        let given = ActorEvent::GivenToRead { recipient, message: open.message.clone() };
        match tx.send(Envelope::new(self.context(self.thread), given)).await {
            Ok(()) => carrying.briefed = Some(open.message.id),
            Err(e) => warn!("Delegate of thread {} could not give a brief: {:#}", self.thread, e),
        }
    }

    /// Gives `text`, what came of `open`, back to the giver where the thread was opened - of the
    /// recipient, classified `classification`, else as the brief -, once: the channel takes the
    /// brief as answered as it records it, or drops it if it refuses.
    async fn give_back(
        &self,
        carrying: &mut Carrying,
        open: &Brief,
        text: String,
        classification: Option<Classification>,
        tx: &dyn EventSender,
    ) {
        let (giver, recipient) = {
            let delegation = self.delegation();
            (delegation.giver, delegation.recipient)
        };
        let classification = classification.unwrap_or_else(|| open.message.classification.clone());
        let message = Message {
            id: MessageId::new(),
            sender_id: recipient,
            sender_name: self.recipient_name.clone(),
            role: MessageRole::User,
            content: Some(text),
            ..Message::new(classification)
        };
        let given = ActorEvent::GivenToRead { recipient: giver, message };
        match tx.send(Envelope::new(self.context(self.origin), given)).await {
            Ok(()) => carrying.answered = Some(open.message.id),
            Err(e) => warn!("Delegate of thread {} could not give back: {:#}", self.thread, e),
        }
    }

    /// The text of the kernel's `key`, with `values`, for what came of a brief.
    fn told(&self, key: &str, values: impl Serialize) -> String {
        self.prompts.render(key, values).unwrap_or_else(|e| {
            warn!("Delegate of thread {} cannot render {}: {}", self.thread, key, e);
            String::new()
        })
    }

    /// Takes in `envelope`: what the recipient says, the end of its turn that answers the open
    /// brief, its leave, and a brief refused.
    async fn take_in(&self, envelope: Envelope, carrying: &mut Carrying, tx: &dyn EventSender) {
        let recipient = self.delegation().recipient;
        let open = self.open().filter(|open| carrying.answered != Some(open.message.id));
        // The end of a turn after the open brief was given: its answer.
        let answers = open.as_ref().and_then(|open| open.given_at).is_some_and(|given_at| {
            envelope.context.position.is_some_and(|position| position > given_at)
        });
        match envelope.event {
            ActorEvent::EventRejected { message: Some(refused), reason, .. } => {
                warn!("Delegate of thread {} was refused a brief: {:?}", self.thread, reason);
                if let Some(open) = open.filter(|open| open.message.id == refused) {
                    let text = self.told("kernel.delegation_not_asked", context! {});
                    self.give_back(carrying, &open, text, None, tx).await;
                }
            }
            // Announced in the name of whoever let it go.
            ActorEvent::ParticipantLeft { participant, .. } if participant == recipient => {
                carrying.gone = true;
                info!("Delegate of thread {}: its recipient left", self.thread);
            }
            _ if envelope.context.sender_id != recipient => {}
            ActorEvent::CompleteMessageGenerated(message) => {
                if let Some(turn) = message.turn_id.filter(|_| message.is_heard()) {
                    carrying.said.entry(turn).or_default().push(message);
                }
            }
            ActorEvent::TurnFinished { turn_id, .. } => {
                let said = carrying.said.remove(&turn_id).unwrap_or_default();
                if let Some(open) = open.filter(|_| answers) {
                    let (text, classification) = self.answer(&said);
                    self.give_back(carrying, &open, text, classification, tx).await;
                }
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                carrying.said.remove(&turn_id);
                if let Some(open) = open.filter(|_| answers) {
                    let cause = reason.cause();
                    let text = self.told("kernel.delegation_given_up", context! { cause });
                    self.give_back(carrying, &open, text, None, tx).await;
                }
            }
            _ => {}
        }
    }

    /// What came of the open brief, of what the turn answering it `said` that others hear: its
    /// words, classified as they are; that it ended without any, classified as the brief.
    fn answer(&self, said: &[Message]) -> (String, Option<Classification>) {
        let texts: Vec<&str> = said.iter().filter_map(|m| m.content.as_deref()).collect();
        if texts.is_empty() {
            return (self.told("kernel.delegation_unanswered", context! {}), None);
        }
        let classification = Classification::join_all(said.iter().map(|m| &m.classification));
        (texts.join("\n\n"), classification)
    }
}

#[async_trait]
impl Actor for DelegateActor {
    fn id(&self) -> ActorId {
        ActorId::Delegate(self.thread)
    }

    fn name(&self) -> Name {
        ActorId::delegate_name()
    }

    /// What it waits for of its thread: what the recipient says, the end of its turn, its leave,
    /// and what of its own was not let in.
    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |envelope| {
            matches!(
                envelope.event,
                ActorEvent::CompleteMessageGenerated(_)
                    | ActorEvent::TurnFinished { .. }
                    | ActorEvent::TurnGivenUp { .. }
                    | ActorEvent::ParticipantLeft { .. }
                    | ActorEvent::EventRejected { .. }
            )
        }
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        info!("Delegate of thread {} started", self.thread);
        let mut carrying = Carrying::default();
        loop {
            self.carry(&mut carrying, &*tx).await;
            tokio::select! {
                () = self.ended.cancelled() => break,
                () = self.changed.notified() => {}
                received = rx.recv() => match received {
                    Some(envelope) => self.take_in(envelope, &mut carrying, &*tx).await,
                    None => break,
                },
            }
        }
        info!("Delegate of thread {} ended", self.thread);
        Ok(())
    }

    /// Where its open brief was given, if it was: what it reads on from as it starts again, so
    /// that the answer to it is not lost however long it did not run.
    async fn heard(&self) -> Option<Heard> {
        self.open().and_then(|open| open.given_at).map(Heard::at)
    }

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    fn stop(&self) {
        self.ended.cancel();
    }

    fn leave(&self) {
        self.ended.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_domain::actor::Rejection;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::Word;
    use caelumora_domain::prompt_catalogue::{KERNEL, KERNEL_PROMPTS};
    use caelumora_domain::turn::GivenUpReason;
    use caelumora_domain::user::UserId;
    use std::collections::VecDeque;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tokio::time;

    /// A delegate of a thread opened at the line, delegated into by a user to an agent, running -
    /// with what the channel's state holds of its delegation, which the test moves on as the
    /// channel would (`sync`).
    struct Running {
        delegate: Arc<DelegateActor>,
        delegation: Delegation,
        to_it: mpsc::Sender<Envelope>,
        given: mpsc::Receiver<Envelope>,
        run: JoinHandle<Result<()>>,
        thread: ThreadId,
        line: ThreadId,
    }

    fn brief(giver: ActorId, text: &str, given_at: Option<Position>) -> Brief {
        let message = Message {
            id: MessageId::new(),
            sender_id: giver,
            content: Some(text.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        Brief { message, given_at }
    }

    fn running(briefs: impl FnOnce(ActorId) -> Vec<Brief>) -> Running {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let (thread, line) = (ThreadId::new(), ThreadId::line(channel));
        let giver = ActorId::User(UserId::new());
        let recipient = ActorId::Agent(AgentId::new());
        let briefs = VecDeque::from(briefs(giver));
        let delegation =
            Delegation { giver, recipient, initiator: giver, briefs, word: Word::NotDue };
        let mut prompts = PromptCatalogue::new();
        prompts.add(KERNEL, KERNEL_PROMPTS).expect("the kernel's texts");
        let delegate = Arc::new(DelegateActor::new(
            thread,
            line,
            session,
            channel,
            delegation.clone(),
            "helper".to_string(),
            Arc::new(prompts),
        ));
        let (to_it, rx) = mpsc::channel(16);
        let (tx, given) = mpsc::channel(16);
        let run = tokio::spawn({
            let delegate = Arc::clone(&delegate);
            async move {
                let rx = Box::new(InMemoryEventReceiver::new(rx));
                delegate.run(rx, Box::new(InMemoryEventSender::new(tx))).await
            }
        });
        Running { delegate, delegation, to_it, given, run, thread, line }
    }

    impl Running {
        /// What it gives next: in which thread, to whom, what, in which chain.
        async fn gives(&mut self) -> (ThreadId, ActorId, Message, ActorId) {
            let envelope = time::timeout(Duration::from_secs(5), self.given.recv())
                .await
                .expect("given in time")
                .expect("running");
            let ActorEvent::GivenToRead { recipient, message } = envelope.event else {
                panic!("gave something else: {:?}", envelope.event);
            };
            (envelope.context.thread_id, recipient, message, envelope.context.initiator_id)
        }

        /// Whether it gives nothing for a while.
        async fn gives_nothing(&mut self) -> bool {
            time::timeout(Duration::from_millis(100), self.given.recv()).await.is_err()
        }

        /// The channel recording what it gave, `recipient` given `message`, at `at`.
        fn recorded(&mut self, recipient: ActorId, message: &Message, at: u64) {
            self.delegation.follow(recipient, message, Position::new(at));
            self.delegate.sync(self.delegation.clone());
        }

        /// `event` of `sender` in the thread, at `at`.
        async fn hears(&self, sender: ActorId, at: u64, event: ActorEvent) {
            let context = EventContext {
                sender_id: sender,
                thread_id: self.thread,
                position: Some(Position::new(at)),
                ..EventContext::default()
            };
            self.to_it.send(Envelope::new(context, event)).await.expect("running");
        }

        /// The recipient saying `text` in its turn `turn`, which then ends, from `at` on.
        async fn answers(&self, turn: TurnId, text: &str, at: u64) {
            let recipient = self.delegation.recipient;
            let said = Message {
                sender_id: recipient,
                role: MessageRole::Assistant,
                content: Some(text.to_string()),
                turn_id: Some(turn),
                ..Message::new(Classification::CHANNEL_DEFAULT)
            };
            let step = Message { role: MessageRole::Tool, ..said.clone() };
            self.hears(recipient, at, ActorEvent::CompleteMessageGenerated(step)).await;
            self.hears(recipient, at + 1, ActorEvent::CompleteMessageGenerated(said)).await;
            let finished = ActorEvent::TurnFinished { turn_id: turn, messages: Vec::new() };
            self.hears(recipient, at + 2, finished).await;
        }
    }

    /// Issue 557: a delegate gives its open brief to the recipient, in its thread, in the
    /// delegating chain's name, once; it takes the recipient's first turn that ends after the
    /// channel recorded the brief given - what others hear of it - and gives it back to the giver
    /// where the thread was opened, once; then the next brief, once the channel took the first as
    /// answered, whose turn it brings back as given up.
    #[tokio::test]
    async fn a_delegate_gives_its_briefs_and_brings_back_the_turns_that_answer_them() {
        let mut delegate =
            running(|giver| vec![brief(giver, "look it up", None), brief(giver, "and this", None)]);
        let (giver, recipient) = (delegate.delegation.giver, delegate.delegation.recipient);
        let (thread, to, first, chain) = delegate.gives().await;
        assert_eq!((thread, to, chain), (delegate.thread, recipient, giver), "in its thread");
        assert!(delegate.gives_nothing().await, "once");

        delegate.answers(TurnId::new(), "before", 2).await;
        delegate.recorded(recipient, &first, 5);
        assert!(delegate.gives_nothing().await, "a turn ended before the brief was given is none");
        delegate.answers(TurnId::new(), "found it", 6).await;
        let (thread, to, answer, chain) = delegate.gives().await;
        assert_eq!((thread, to, answer.sender_id, chain), (delegate.line, giver, recipient, giver));
        assert_eq!(answer.content.as_deref(), Some("found it"), "what others hear of the turn");
        assert!(delegate.gives_nothing().await, "given back once, until the channel follows");

        delegate.recorded(giver, &answer, 9);
        let (_, _, second, _) = delegate.gives().await;
        assert_eq!(second.content.as_deref(), Some("and this"), "the next brief");
        delegate.recorded(recipient, &second, 10);
        let given_up =
            ActorEvent::TurnGivenUp { turn_id: TurnId::new(), reason: GivenUpReason::Retried };
        delegate.hears(recipient, 11, given_up).await;
        let (_, _, message, _) = delegate.gives().await;
        assert!(
            message.content.as_deref().is_some_and(|text| text.contains("given up")),
            "that it was given up: {:?}",
            message.content
        );
        delegate.run.abort();
    }

    /// Issue 557: a brief the channel refuses - its recipient gone before it was given - is given
    /// back as one its recipient could not be asked.
    #[tokio::test]
    async fn a_brief_refused_comes_back_as_not_asked() {
        let mut delegate = running(|giver| vec![brief(giver, "look it up", None)]);
        let (_, _, first, _) = delegate.gives().await;
        let refused = ActorEvent::EventRejected {
            rejected: "GivenToRead".to_string(),
            message: Some(first.id),
            reason: Rejection::Invalid,
        };
        delegate.hears(ActorId::System, 3, refused).await;
        let (thread, to, message, _) = delegate.gives().await;
        assert_eq!((thread, to), (delegate.line, delegate.delegation.giver), "back to the giver");
        assert_eq!(message.content.as_deref(), Some("It could not be asked."));
        delegate.run.abort();
    }

    /// Issue 557: a delegate built again with a brief given reads on from where it was given and
    /// gives it not again; once its recipient left - announced in the name of whoever let it go -,
    /// it gives back that it did, for the open brief and for each that waited behind it.
    #[tokio::test]
    async fn a_delegate_started_again_reads_on_and_gives_back_what_its_recipient_left() {
        let at = Position::new(7);
        let mut delegate = running(|giver| {
            vec![brief(giver, "look it up", Some(at)), brief(giver, "and this", None)]
        });
        assert_eq!(
            delegate.delegate.heard().await.map(|heard| heard.next()),
            Some(at),
            "where its open brief was given"
        );
        assert!(delegate.gives_nothing().await, "given once");
        let (giver, recipient) = (delegate.delegation.giver, delegate.delegation.recipient);
        let left = ActorEvent::ParticipantLeft {
            participant: recipient,
            name: "helper".to_string(),
            until: Position::new(9),
            out_of_channel: false,
        };
        delegate.hears(giver, 9, left).await;
        for brief in ["look it up", "and this"] {
            let (thread, to, message, _) = delegate.gives().await;
            assert_eq!((thread, to), (delegate.line, giver), "back to the giver, for {brief}");
            assert_eq!(message.content.as_deref(), Some("It left before it answered."));
            delegate.recorded(giver, &message, 10);
        }
        assert!(delegate.gives_nothing().await, "and nothing more to one gone");
        delegate.run.abort();
    }
}
