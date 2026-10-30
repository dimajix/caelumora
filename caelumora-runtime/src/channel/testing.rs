//! What the tests of a running channel share (issue 370): those of the
//! runtime, which run scripted agents (`testing::ScriptedAgents`), and
//! those of the agents' logic, which run real ones - a channel of alice
//! and her agent, started, and what is waited for in it. Behind the
//! feature `test-support`, as `crate::testing` is.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use crate::actor::Actor;
use crate::channel::ChannelLoop;
use crate::command::CommandError;
use crate::controller::SessionController;
use crate::runtime::{ActorRuntime, ChannelRuntime, RuntimeManager};
use crate::view::AgentContextView;
use anyhow::{Error, Result};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::testing::stored_agent;
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::approval::{ApprovalStrategy, Approver};
use caelumora_model::chain::Position;
use caelumora_model::channel::testing::{founded, make_channel};
use caelumora_model::channel::{Channel, InternalActor, Participant};
use caelumora_model::history::testing::{items_of, messages_of};
use caelumora_model::history::{HistoryEntry, Reader};
use caelumora_model::message::Message;
use caelumora_model::message::MessageId;
use caelumora_model::security::{AccessPolicy, ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::session::testing::bare_session;
use caelumora_model::session::SessionId;
use caelumora_model::store::Persistence;
use caelumora_model::store::StoreResult;
use caelumora_model::user::testing::{make_user, record_users};
use caelumora_model::user::UserId;
use caelumora_model::work::TurnId;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;
use tokio::time;

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
pub fn command_error(error: &Error) -> &CommandError {
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
    pub running: JoinHandle<Result<()>>,
}

/// Alice and her agent `name` of `personality`, stored in `store`: her context, the agent and her.
pub async fn alice_and_an_agent(
    store: &Persistence,
    name: &str,
    personality: Personality,
) -> (SecurityContext, AgentId, UserId) {
    let stores = store.entity_stores().unwrap();
    let alice = make_user("alice");
    record_users(&stores, &[&alice]).await;
    let context =
        SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let agent = stored_agent(&stores, name, personality, context.initiator_id).await;
    (context, agent.id, alice.id)
}

/// The channel of `alice` and `agent`, with its approver, in `runtime`,
/// built but not running.
pub async fn channel_of_alice_and(
    runtime: Arc<RuntimeManager>,
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
    runtime
        .entity_stores()
        .channels
        .create(&SecurityContext::system(), &mut channel)
        .await
        .unwrap();
    runtime.state_store().put_channel_state(channel.id, &mut founded(&channel)).await.unwrap();
    let session_id = channel.session_id;
    ChannelLoop::new(ChannelRuntime::outside_a_session(runtime, bare_session(session_id), channel))
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

/// What the tests of a channel read of it and do to it past what its
/// users may (issue 370): its internals, behind the feature `test-support`
/// alone, under names of their own - the channel's API is not widened by
/// them.
impl ChannelLoop {
    /// The actor of `actor` in the channel (`ChannelLoop::actor`).
    pub fn actor_of(&self, actor: ActorId) -> Option<Arc<dyn Actor>> {
        self.actor(actor)
    }

    /// The messages of the history it holds in memory, as they stand: what
    /// a test of the channel looks at.
    pub(crate) async fn history(&self) -> Vec<Message> {
        messages_of(&self.state.lock().await.history)
    }

    /// The whole history, whoever may see it (`ChannelLoop::history`).
    pub async fn whole_history(&self) -> Vec<Message> {
        self.history().await
    }

    /// Records `messages` as said in the channel - in its history, stored -
    /// without routing them: what an actor that starts reads from there
    /// (issue 223).
    pub async fn record_said(&self, messages: Vec<Message>) {
        let mut state = self.state.lock().await;
        state.history.extend(messages);
        self.state_store
            .put_channel_state(self.id, &mut state)
            .await
            .expect("the history is stored");
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

    /// The ranges of positions in which `participant` was given what went to
    /// every member (`HistoryStore::memberships`, issue 417).
    pub async fn memberships(&self, participant: ActorId) -> Vec<Range<Position>> {
        self.history_store.memberships(self.id, participant).await.expect("the history is read")
    }

    /// The entries of the whole history, its messages folded as they stand.
    pub async fn history_entries(&self) -> Vec<HistoryEntry> {
        items_of(&self.state.lock().await.history)
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

    /// The runtime of `actor` in this channel, attached to it, as its turns
    /// act through it - for tests, where no actor runs.
    pub(crate) fn new_actor_runtime(&self, actor: ActorId) -> ActorRuntime {
        self.runtime.new_actor_runtime(actor)
    }
}
