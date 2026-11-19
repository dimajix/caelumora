//! What the tests of a running channel share (issue 370): those of the kernel, which run scripted
//! agents (`testing::ScriptedAgents`), and those of the agents' logic, which run real ones - a
//! channel of alice and her agent, started, and what is waited for in it. Behind the feature
//! `test-support`, as `crate::testing` is.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use crate::channel::thread::{Phase, Thread};
use crate::channel::ChannelLoop;
use crate::channel::ResolvedParticipant;
use crate::controller::SessionController;
use crate::kernel::{ChannelRuntime, Kernel, ParticipantRuntime};
use crate::testing::shown;
use crate::view::AgentContextView;
use anyhow::{Error, Result};
use caelumora_api::actor::Actor;
use caelumora_api::command::CommandError;
use caelumora_api::transport::{EventReceiver, EventSender};
use caelumora_domain::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_domain::agent::{AgentChannelState, AgentId, Personality};
use caelumora_domain::approval::{ApprovalStrategy, Approver};
use caelumora_domain::chain::Position;
use caelumora_domain::channel::testing::{founded, make_channel};
#[cfg(test)]
use caelumora_domain::channel::ChannelId;
use caelumora_domain::channel::{
    Channel, Delegation, InternalActor, Opening, Participant, ThreadId, ThreadOrigin, ThreadState,
};
use caelumora_domain::content::ContentId;
use caelumora_domain::history::testing::{items_of, messages_of};
use caelumora_domain::history::{Attended, Audience, History, HistoryEntry, Reader, Recorded};
use caelumora_domain::message::Message;
use caelumora_domain::message::MessageId;
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::session::testing::bare_session;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResult;
use caelumora_domain::user::testing::make_user;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::Name;
use caelumora_domain::work::TurnId;
use caelumora_security::{AccessPolicy, ActionType, Policies, SecurityContext};
use caelumora_store::testing::record_users;
use caelumora_store::testing::stored_agent;
use caelumora_store::{EntityStores, Persistence};
use std::collections::HashMap;
use std::convert::Infallible;
use std::mem;
use std::sync;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;
use tokio::time;

/// An actor that sends the envelopes it is given as it starts, as they are written - in whatever
/// name, with whatever hops - and waits: what no actor of the kernel can, which builds its
/// envelopes itself (`Answering`). For the tests of what the channel does with such envelopes
/// (issue 372), and in place of a delegate until its own actor is built (issue 557).
#[derive(Debug)]
pub(crate) struct Forger {
    id: ActorId,
    envelopes: sync::Mutex<Vec<Envelope>>,
}

impl Forger {
    pub(crate) fn sending(id: ActorId, envelopes: Vec<Envelope>) -> Arc<Self> {
        Arc::new(Self { id, envelopes: sync::Mutex::new(envelopes) })
    }
}

#[async_trait::async_trait]
impl Actor for Forger {
    fn id(&self) -> ActorId {
        self.id
    }
    fn name(&self) -> Name {
        Name::new("talker").expect("a name")
    }
    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }
    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        let envelopes = mem::take(&mut *self.envelopes.lock().unwrap());
        for envelope in envelopes {
            let _ = tx.send(envelope).await;
        }
        while rx.recv().await.is_some() {}
        Ok(())
    }
    fn state(&self) -> ActorState {
        ActorState::Idle
    }
    fn stop(&self) {}
    fn leave(&self) {}
}

/// Decides by a rule over the action alone, to refuse one thing and
/// allow the rest.
#[derive(Debug)]
pub struct Policy(pub fn(ActionType<'_>) -> bool);

#[async_trait::async_trait]
impl AccessPolicy for Policy {
    async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
        (self.0)(action)
    }
}

/// The `CommandError` `error` is, which a test expects it to be.
pub fn as_command_error(error: &Error) -> &CommandError {
    error.downcast_ref().unwrap_or_else(|| panic!("expected a CommandError, got {error:#}"))
}

/// A running channel with alice and her agent, as `start` leaves it after
/// alice said "hello": the channel, alice's context, the agent and its
/// actor, what the channel takes in from then on (`ChannelLoop::observe`)
/// and its running task.
pub struct Stuck {
    pub channel: Arc<ChannelLoop>,
    pub context: SecurityContext,
    pub agent: AgentId,
    pub helper: Arc<dyn Actor>,
    pub events: broadcast::Receiver<Envelope>,
    pub running: JoinHandle<Result<Infallible>>,
}

/// Alice and her agent `name` of `personality`, stored in `store`: her context, the agent and her.
pub async fn alice_and_an_agent(
    store: &Persistence,
    name: &str,
    personality: Personality,
) -> (SecurityContext, AgentId, UserId) {
    let stores = EntityStores::open(store, &Policies::permitting_all()).unwrap();
    let alice = make_user("alice");
    record_users(&stores, &[&alice]).await;
    let context =
        SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let agent = stored_agent(&stores, name, personality, context.initiator_id).await;
    (context, agent.id, alice.id)
}

/// The channel of `alice` and `agent`, with its approver, in `kernel`, built but not running.
pub async fn channel_of_alice_and(
    kernel: Arc<Kernel>,
    alice: UserId,
    agent: AgentId,
) -> Arc<ChannelLoop> {
    let mut channel = Channel {
        participants: vec![Participant::User(alice), Participant::Agent(agent)],
        internal_actors: vec![InternalActor::Approver(Approver::new(ApprovalStrategy::Any))],
        security_attributes: SecurityAttributes::owned_by(ActorId::User(alice)),
        ..make_channel(SessionId::new(), "main")
    };
    // Stored, so that leaving it can be stored too, with its participants'
    // joins (issue 420).
    kernel.entity_stores().channels.create(&SecurityContext::system(), &mut channel).await.unwrap();
    kernel.state_store().put_channel_state(channel.id, &mut founded(&channel)).await.unwrap();
    let session_id = channel.session_id;
    ChannelLoop::new(ChannelRuntime::outside_a_session(kernel, bare_session(session_id), channel))
        .await
        .unwrap()
}

/// Waits for `actor` to reach `wanted`.
pub async fn settled(actor: &Arc<dyn Actor>, wanted: ActorState) {
    time::timeout(Duration::from_secs(5), async {
        while actor.state() != wanted {
            time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the agent does not reach {wanted:?}, it is {:?}", actor.state()));
}

/// Runs `built` and has alice say "hello".
pub async fn start(
    (channel, context, agent, alice): (Arc<ChannelLoop>, SecurityContext, AgentId, UserId),
) -> Stuck {
    let events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(Some("hello".to_string()), None, Vec::new())
        .await
        .unwrap();
    let helper = channel.actor(ActorId::Agent(agent)).unwrap();
    Stuck { channel, context, agent, helper, events, running }
}

/// What `agent` answered next.
pub async fn answered(events: &mut Receiver<Envelope>, agent: AgentId) -> Vec<Message> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                    break messages;
                }
            }
        }
    })
    .await
    .expect("the agent answers")
}

/// The user `context` acts for.
pub fn alice_of(context: &SecurityContext) -> UserId {
    match context.initiator_id {
        ActorId::User(user) => user,
        other => panic!("{other} is no user"),
    }
}

/// What the tests make of a participant past its own API: an approver, to look an actor up by.
#[cfg(test)]
impl ResolvedParticipant {
    /// An approver of channel `channel_id` of session `session_id` - in
    /// tests, as the participant of an actor of whatever kind the channel
    /// only looks up.
    pub(crate) fn approver(session_id: SessionId, channel_id: ChannelId) -> Self {
        Self::internal(
            &InternalActor::Approver(Approver::new(ApprovalStrategy::default())),
            session_id,
            channel_id,
        )
    }
}

/// What the tests of a channel read of it and do to it past what its
/// users may (issue 370): its internals, behind the feature `test-support`
/// alone, under names of their own - the channel's API is not widened by
/// them.
impl ChannelLoop {
    /// A thread at the line, delegated into as `delegation` says, whose delegate is forged
    /// (issue 557): an actor in its place that sends what `envelopes` makes for the thread as it
    /// is written (`Forger`) - what the channel admits of a delegate, tested without one. The
    /// thread is held and kept in the channel's state at once, as the channel builds it as it
    /// starts, opened by nobody and announced nowhere. Returns the thread.
    pub async fn forge_delegate(
        &self,
        delegation: Delegation,
        envelopes: impl FnOnce(ThreadId) -> Vec<Envelope>,
    ) -> ThreadId {
        let thread = ThreadId::new();
        let origin = ThreadOrigin { thread: ThreadId::line(self.id), message: MessageId::new() };
        let opening = Opening { origin, by: ActorId::System };
        let held =
            ThreadState { opening: Some(opening), closed: false, delegation: Some(delegation) };
        self.state.lock().await.threads.insert(thread, held);
        let moderator = self.moderator().await.expect("a moderator");
        let opened = Arc::new(Thread::new(moderator, Some(opening), Phase::Open));
        self.threads.rcu(|threads| {
            let mut threads = HashMap::clone(threads);
            threads.insert(thread, Arc::clone(&opened));
            threads
        });
        let delegate = ActorId::Delegate(thread);
        let resolved = ResolvedParticipant::delegate(thread, self.session_id, self.id);
        let forger = Forger::sending(delegate, envelopes(thread));
        self.adopt(forger, resolved, thread, None, None).await;
        thread
    }

    /// Every event the channel takes in from now on (see `observed`).
    pub fn observe(&self) -> broadcast::Receiver<Envelope> {
        self.observed.subscribe()
    }

    /// The context of an event made for the channel by nobody in particular: in its session, in
    /// it and in its line (issue 557) - what a test sets the sender and the rest of in.
    pub fn event_context(&self) -> EventContext {
        EventContext::new(self.session_id, self.id, ActorId::None, String::new())
    }

    /// The actor of `actor` in the channel (`ChannelLoop::actor`).
    pub fn actor_of(&self, actor: ActorId) -> Option<Arc<dyn Actor>> {
        self.actor(actor)
    }

    /// The messages of the history it holds in memory, as they stand: what
    /// a test of the channel looks at.
    pub(crate) async fn history(&self) -> Vec<Message> {
        messages_of(&self.state.lock().await.history)
    }

    /// What `read` makes of the history the channel holds in memory, as it was recorded: its
    /// entries with their senders and recipients, where it begins and ends.
    pub async fn reading_history<R>(&self, read: impl FnOnce(&History) -> R) -> R {
        read(&self.state.lock().await.history)
    }

    /// The whole history, whoever may see it (`ChannelLoop::history`).
    pub async fn whole_history(&self) -> Vec<Message> {
        self.history().await
    }

    /// Records `messages` as said in the channel - in its history, in its line (issue 557),
    /// stored - without routing them: what an actor that starts reads from there (issue 223).
    pub async fn record_said(&self, messages: Vec<Message>) {
        let mut state = self.state.lock().await;
        for message in messages {
            let sender = message.sender_id;
            let context =
                EventContext { sender_id: sender, initiator_id: sender, ..self.event_context() };
            let said = Envelope::new(context, ActorEvent::CompleteMessageGenerated(message));
            let recorded = Recorded::of(&said, sender, Audience::Everybody).expect("it is said");
            state.history.enter(recorded);
        }
        self.state_store
            .put_channel_state(self.id, &mut state)
            .await
            .expect("the history is stored");
    }

    /// Has `user` say `text`, with the attachments `attachments` they uploaded for it, and waits
    /// until the channel recorded it in its history: the id of the message.
    ///
    /// # Panics
    /// If `user` takes no part, or the message is not sent, not shown to them or not recorded
    /// within five seconds.
    pub async fn say_as(&self, user: UserId, text: &str, attachments: Vec<ContentId>) -> MessageId {
        let actor = self.user_actor(user).expect("the user takes part");
        let mut watch = actor.watch();
        actor.send_message(Some(text.to_string()), None, attachments).await.expect("sent");
        let ActorEvent::CompleteMessageGenerated(said) =
            shown(&mut watch, "CompleteMessageGenerated").await
        else {
            unreachable!()
        };
        // Shown as it is let in, recorded right after.
        time::timeout(Duration::from_secs(5), async {
            while !self.visible_messages(ActorId::User(user)).await.iter().any(|m| m.id == said.id)
            {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in the history");
        said.id
    }

    /// Runs the channel until the stored state of `agent` fulfils `done` -
    /// what it takes in as it starts, read from the history behind its inbox
    /// (issue 223) -, then stops it; that state.
    ///
    /// # Panics
    /// If it does not within five seconds.
    pub async fn run_until(
        self: &Arc<Self>,
        agent: AgentId,
        done: impl Fn(&AgentChannelState) -> bool,
    ) -> AgentChannelState {
        let running = tokio::spawn({
            let channel = Arc::clone(self);
            async move { channel.run().await }
        });
        let state = time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(state) = self.stored_agent_channel_state(agent).await {
                    if done(&state) {
                        break state;
                    }
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the agent's state comes to stand so");
        running.abort();
        state
    }

    /// The messages `participant` sees of the history, read from the store
    /// as a view reads them (issues 379, 417).
    pub async fn visible_messages(&self, participant: ActorId) -> Vec<Message> {
        self.visible_entries(participant)
            .await
            .iter()
            .filter_map(HistoryEntry::said)
            .cloned()
            .collect()
    }

    /// The entries `participant` sees of the history, as `visible_messages`
    /// reads them: who joined or left among them (issue 408).
    pub async fn visible_entries(&self, participant: ActorId) -> Vec<HistoryEntry> {
        self.history_store
            .page(self.id, &Reader::Participant(participant), Position::START, None, 10_000, None)
            .await
            .expect("the history is read")
            .entries()
            .cloned()
            .collect()
    }

    /// Where `participant` was given what went to every participant (`HistoryStore::attendance`,
    /// issues 417, 557).
    pub async fn attendance(&self, participant: ActorId) -> Vec<Attended> {
        self.history_store.attendance(self.id, participant).await.expect("the history is read")
    }

    /// The entries of the whole history, its messages folded as they stand.
    pub async fn history_entries(&self) -> Vec<HistoryEntry> {
        items_of(&self.state.lock().await.history)
    }

    /// The entries of the whole history once `done` holds of them, waited for as `run` records -
    /// at most five seconds, after which the test fails.
    pub async fn history_once(&self, done: impl Fn(&[HistoryEntry]) -> bool) -> Vec<HistoryEntry> {
        time::timeout(Duration::from_secs(5), async {
            loop {
                let entries = self.history_entries().await;
                if done(&entries) {
                    return entries;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recorded as wanted")
    }

    /// The last entry of the history as it was recorded, unfolded.
    pub async fn last_history_entry(&self) -> Option<HistoryEntry> {
        self.state.lock().await.history.last().map(|node| node.entry.entry.clone())
    }

    /// The message `id` of the history (`ChannelLoop::message`).
    pub async fn message_of(&self, id: MessageId) -> Option<Message> {
        self.message(id).await
    }

    /// The controller of the channel's session, as its actors read it
    /// (`ChannelRuntime::session_controller`): what a test changes of the
    /// session where the channel runs without its session's loop.
    pub fn session_controller_of(&self) -> Arc<SessionController> {
        Arc::clone(self.runtime.session_controller())
    }

    /// The turn under way of the participant `actor`
    /// (`ChannelLoop::turn_under_way`).
    pub fn turn_of(&self, actor: ActorId) -> Option<TurnId> {
        self.turn_under_way(actor)
    }

    /// Stops every actor, as stopping the channel with its session does
    /// (`ChannelLoop::stop_actors`).
    pub async fn stop_all_actors(&self) {
        self.stop_actors().await;
    }

    /// Has every actor leave, as deleting the channel does
    /// (`ChannelLoop::leave_actors`).
    pub async fn leave_all_actors(&self) {
        self.leave_actors().await;
    }

    /// The state of `agent` in this channel, as stored.
    ///
    /// # Errors
    /// Whatever the `StateStore` reports.
    pub async fn stored_agent_channel_state(
        &self,
        agent: AgentId,
    ) -> StoreResult<AgentChannelState> {
        self.state_store.get_agent_channel_state(agent, self.id).await
    }

    /// What `agent` saw in the channel, as a viewer who may read it is shown
    /// it: its state and its transcript as stored, and the channel's history
    /// as it saw it (`AgentContextView`).
    pub async fn agent_context(&self, agent: AgentId) -> AgentContextView {
        let state = self.stored_agent_channel_state(agent).await.expect("the state is read");
        AgentContextView::of(
            agent,
            self.id,
            state,
            Arc::clone(&self.history_store),
            Arc::clone(self.runtime.transcript_store()),
            Arc::clone(self.runtime.prompts()),
        )
    }

    /// The runtime of this channel built again, as after a restart
    /// (`ChannelRuntime::restarted`).
    pub fn restarted_runtime(&self) -> ChannelRuntime {
        self.runtime.restarted()
    }

    /// The runtime of `actor` in this channel, attached to it, as its turns act through it - for
    /// tests, where no actor runs: the participant's, acting in its context as the controllers
    /// hold it (issue 516), or, for an actor that takes no part, one in its own name without roles.
    pub(crate) fn new_participant_runtime(&self, actor: ActorId) -> ParticipantRuntime {
        match self.actors.load().get(&actor) {
            Some(taking_part) => {
                self.runtime.participant_runtime(&taking_part.participant, taking_part.thread)
            }
            None => self.runtime.new_participant_runtime(actor),
        }
    }
}
