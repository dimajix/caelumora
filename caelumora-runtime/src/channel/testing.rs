//! What the tests of a running channel share (issue 370): those of the
//! runtime, which run scripted agents (`testing::ScriptedAgents`), and
//! those of the agents' logic, which run real ones - a channel of alice
//! and her agent, started, and what is waited for in it. Behind the
//! feature `test-support`, as `crate::testing` is.

use caelumora_model::history::testing::{items_of, messages_of};
use crate::actor::Actor;
use crate::channel::ChannelLoop;
use crate::command::CommandError;
use crate::runtime::{ActorRuntime, ChannelRuntime, RuntimeManager};
use crate::view::AgentContextView;
use crate::session::testing::{bare_session, user};
use anyhow::{Result, Error};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::history::{HistoryEntry, Reader};
use caelumora_model::message::MessageId;
use caelumora_model::store::history::every;
use caelumora_model::store::StoreResult;
use caelumora_model::work::TurnId;
use caelumora_model::channel::{Channel, ChannelId, HistoryStart, Participant, moderation_components};
use caelumora_model::component::ComponentConfig;
use caelumora_model::message::Message;
use caelumora_model::security::{AccessPolicy, ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::user::UserId;
use caelumora_model::value::name::{Name, ScopedName};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
use caelumora_model::confidentiality::{Classification, Visibility};
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;

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

/// Alice and her agent `helper` of a personality with `components`,
/// stored in `store`: her context, the agent and her.
pub async fn alice_and_an_agent(store: &MemoryStore, components: ComponentConfig) -> (SecurityContext, AgentId, UserId) {
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::scope::Scope;

    let personality = Personality {
        description: None,
        introduction: None,
        reasoning_visibility: Visibility::Channel,
        id: PersonalityId::new(),
        name: ScopedName::new("plain", Default::default()).unwrap(),
        kind: Default::default(),
        roles: None,
        components,
        security_attributes: Default::default(),
        deleted: None,
    };
    store.entity_stores().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
    let alice = user("alice");
    let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let agent = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, context.initiator_id);
    store.entity_stores().users.create(&SecurityContext::system(), &alice).await.unwrap();
    store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();
    (context, agent.id, alice.id)
}

/// The channel of `alice` and `agent`, with its approver, in `runtime`,
/// built but not running.
pub async fn channel_of_alice_and(runtime: Arc<RuntimeManager>, alice: UserId, agent: AgentId) -> Arc<ChannelLoop> {
    let channel = Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId::new(),
        session_id: SessionId::new(),
        name: Name::new("main").unwrap(),
        components: moderation_components(None, Some("broadcast_policy")),
        participants: vec![
            Participant::User(alice),
            Participant::Agent(agent),
            Participant::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any }),
        ],
        security_attributes: SecurityAttributes::default(),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        deleted: None,
        former: Vec::new(),
    };
    // Stored, so that leaving it can be stored too.
    runtime.entity_stores().channels.create(&SecurityContext::system(), &channel).await.unwrap();
    let session_id = channel.session_id;
    ChannelLoop::new(ChannelRuntime::outside_a_session(runtime, bare_session(session_id), channel)).await.unwrap()
}

/// Waits for `actor` to reach `wanted`.
pub async fn settled(actor: &Arc<dyn Actor>, wanted: ActorState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while actor.state() != wanted { tokio::time::sleep(Duration::from_millis(10)).await }
    }).await.unwrap_or_else(|_| panic!("the agent does not reach {wanted:?}, it is {:?}", actor.state()));
}

/// Runs `built` and has alice say "hello".
pub async fn start((channel, context, agent, alice): (Arc<ChannelLoop>, SecurityContext, AgentId, UserId)) -> Stuck {
    let events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
    channel.user_actor(alice).unwrap().send_message("hello".to_string(), None).await.unwrap();
    let helper = channel.actor(ActorId::Agent(agent)).unwrap();
    Stuck { channel, context, agent, helper, events, running }
}

/// What `agent` answered next.
pub async fn answered(events: &mut Receiver<Envelope>, agent: AgentId) -> Vec<Message> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event { break messages }
            }
        }
    }).await.expect("the agent answers")
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
    /// without routing them: what an actor that joins finds there
    /// (`Actor::catch_up`).
    pub async fn record_said(&self, messages: Vec<Message>) {
        let mut state = self.state.lock().await;
        state.history.extend(messages);
        self.state_store.put_channel_state(self.id, &mut state).await.expect("the history is stored");
    }

    /// The messages `participant` sees of the history, read from the store
    /// from where its view begins, as a view reads them (issue 379).
    pub async fn visible_messages(&self, participant: ActorId) -> Vec<Message> {
        self.visible_entries(participant).await.iter().filter_map(HistoryEntry::said).cloned().collect()
    }

    /// The entries `participant` sees of the history, as `visible_messages`
    /// reads them: who joined or left among them (issue 408).
    pub async fn visible_entries(&self, participant: ActorId) -> Vec<HistoryEntry> {
        let Some(from) = self.visible_from(participant).await.expect("the history is read") else { return Vec::new() };
        self.history_store.page(self.id, &Reader::Participant(participant), from, None, 10_000, &every).await.expect("the history is read").items
    }

    /// Where `participant`'s view of the history begins (`Channel::history_start_of`).
    pub fn history_start_of(&self, participant: ActorId) -> Option<HistoryStart> {
        self.channel.load().history_start_of(participant)
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
    pub async fn stored_agent_channel_state(&self, agent: AgentId) -> StoreResult<AgentChannelState> {
        self.state_store.get_agent_channel_state(agent, self.id).await
    }

    /// What `agent` saw in the channel, as a viewer who may read it is shown
    /// it: its state and its transcript as stored, and the channel's history
    /// as it saw it, from where its view begins (`AgentContextView`).
    pub async fn agent_context(&self, agent: AgentId) -> AgentContextView {
        let state = self.stored_agent_channel_state(agent).await.expect("the state is read");
        let from = self.visible_from(ActorId::Agent(agent)).await.expect("the history is read");
        AgentContextView::of(agent, self.id, state, from, Arc::clone(&self.history_store), Arc::clone(self.runtime.transcript_store()))
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
