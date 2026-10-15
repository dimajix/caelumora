//! An agent whose answers are scripted (issue 370): an actor of a kind of
//! agent, registered in place of the agents' plugin's (`ScriptedAgents`),
//! with nothing of an agent's logic - no model, no tools, no turn - for the
//! tests of what a channel, a session and the views do with an agent among
//! their participants. It tells where it stands, begins and ends its answers
//! as an agent does, and keeps what it hears.

use crate::actor::{Actor, AgentKinds, Answering};
use crate::controller::{AgentController, ChannelController};
use crate::plugin::Plugin;
use crate::runtime::{ActorRuntime, RuntimeBuilder};
use crate::transport::{EventReceiver, EventSender};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::{Personality, PersonalityKind};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::security::SecurityContext;
use caelumora_model::turn::GivenUpReason;
use caelumora_model::value::name::Name;
use caelumora_model::work::TurnId;
use chrono::Utc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

/// What a scripted agent does when it is asked to respond.
#[derive(Debug, Clone)]
pub enum Script {
    /// Answers every request at once with this text, in a turn of its own.
    Answers(String),
    /// Begins a turn and stays in it until it is cancelled, asked for again
    /// or stopped: what a cancel, a retry or a stop has to end.
    Hangs,
}

/// Scripted agents in place of the agents' plugin's, as a plugin: every
/// agent, of either kind, is a `ScriptedAgent` that follows the script.
#[derive(Debug, Clone)]
pub struct ScriptedAgents(pub Script);

impl Plugin for ScriptedAgents {
    fn name(&self) -> &'static str {
        "scripted_agents"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
        let kinds: &mut AgentKinds = runtime.agent_kinds();
        for kind in [PersonalityKind::Internal, PersonalityKind::External] {
            let script = self.0.clone();
            kinds.register(kind, &[], move |channel, agent, personality, runtime| {
                let script = script.clone();
                async move { Ok(Box::new(ScriptedAgent::new(&channel, &agent, &personality, runtime, script)) as Box<dyn Actor>) }
            });
        }
        Ok(())
    }
}

/// Why a scripted agent was interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interrupt {
    Cancel,
    Retry,
    Stop,
}

/// An agent whose answers are scripted.
#[derive(Debug)]
pub struct ScriptedAgent {
    id: ActorId,
    name: Name,
    /// In whose name it acts, as an agent of its owner, limited as its
    /// personality says.
    security: SecurityContext,
    runtime: ActorRuntime,
    script: Script,
    state: Mutex<ActorState>,
    /// The turn under way, if one is.
    turn: Mutex<Option<TurnId>>,
    /// Everything it was given, in order.
    heard: Mutex<Vec<Envelope>>,
    /// Whether it was woken (`wakeup`).
    woken: AtomicBool,
    /// What `cancel`, `retry_turn` and `stop` asked for, taken by `run`.
    interrupt: Mutex<Option<Interrupt>>,
    interrupted: Notify,
}

impl ScriptedAgent {
    fn new(channel: &ChannelController, agent: &AgentController, personality: &Personality, runtime: ActorRuntime, script: Script) -> Self {
        let channel = channel.load();
        Self {
            id: ActorId::Agent(agent.id()),
            name: agent.load().name.name.clone(),
            security: agent.security_context(channel.session_id, channel.id, personality.limiting_roles()),
            runtime,
            script,
            state: Mutex::new(ActorState::Idle),
            turn: Mutex::new(None),
            heard: Mutex::new(Vec::new()),
            woken: AtomicBool::new(false),
            interrupt: Mutex::new(None),
            interrupted: Notify::new(),
        }
    }

    /// Everything it was given so far, in order.
    pub fn heard(&self) -> Vec<Envelope> {
        self.heard.lock().map(|heard| heard.clone()).unwrap_or_default()
    }

    /// Whether it was woken (`Actor::wakeup`) since it was built.
    pub fn woken(&self) -> bool {
        self.woken.load(Ordering::SeqCst)
    }

    /// Answers `request` as the script says. Returns whether it was told to
    /// stop meanwhile.
    async fn respond(&self, request: &Envelope, output: &dyn EventSender) -> bool {
        let answering = Answering::to(request, self.id, self.name.clone(), &self.security);
        let turn_id = TurnId::new();
        self.set_turn(Some(turn_id));
        self.go_to(ActorState::Planning, output).await;
        answering.send_all(vec![ActorEvent::ResponseStarted { turn_id }], output).await;
        let (events, stopped) = match &self.script {
            Script::Answers(text) => {
                let message = self.message(text, turn_id);
                (vec![ActorEvent::CompleteMessageGenerated(message.clone()), ActorEvent::TurnFinished { turn_id, messages: vec![message] }], false)
            }
            Script::Hangs => match self.interrupted().await {
                Interrupt::Cancel => (vec![ActorEvent::TurnFinished { turn_id, messages: Vec::new() }], false),
                Interrupt::Retry => (vec![ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Retried }], false),
                Interrupt::Stop => (vec![ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Stopped }], true),
            },
        };
        self.set_turn(None);
        answering.send_all(events, output).await;
        self.go_to(ActorState::Idle, output).await;
        stopped
    }

    /// Its message saying `text`, of the turn `turn_id`, classified as its
    /// channel is now.
    fn message(&self, text: &str, turn_id: TurnId) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: Some(text.to_string()),
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id,
            sender_name: self.name.to_string(),
            timestamp: Utc::now(),
            turn_id: Some(turn_id),
            ..Message::new(self.runtime.channel_classification().unwrap_or_default())
        }
    }

    /// Waits for the next interrupt and takes it.
    async fn interrupted(&self) -> Interrupt {
        loop {
            if let Some(interrupt) = self.take_interrupt() {
                return interrupt;
            }
            self.interrupted.notified().await;
        }
    }

    fn take_interrupt(&self) -> Option<Interrupt> {
        self.interrupt.lock().ok().and_then(|mut interrupt| interrupt.take())
    }

    fn ask(&self, interrupt: Interrupt) {
        if let Ok(mut asked) = self.interrupt.lock() {
            *asked = Some(interrupt);
        }
        self.interrupted.notify_one();
    }

    fn set_turn(&self, turn: Option<TurnId>) {
        if let Ok(mut current) = self.turn.lock() {
            *current = turn;
        }
    }

    async fn go_to(&self, state: ActorState, output: &dyn EventSender) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
        Answering::own(self.id, self.name.clone(), &self.security).send_all(vec![ActorEvent::StateChanged(state)], output).await;
    }
}

#[async_trait]
impl Actor for ScriptedAgent {
    fn id(&self) -> ActorId {
        self.id
    }

    fn name(&self) -> Name {
        self.name.clone()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        loop {
            let received = tokio::select! {
                received = rx.recv() => received,
                () = self.interrupted.notified() => {
                    // Outside a turn, only a stop ends anything.
                    if self.take_interrupt() == Some(Interrupt::Stop) {
                        break;
                    }
                    continue;
                }
            };
            let Some(envelope) = received else { break };
            if let Ok(mut heard) = self.heard.lock() {
                heard.push(envelope.clone());
            }
            if matches!(envelope.event, ActorEvent::ResponseRequested) && self.respond(&envelope, &*tx).await {
                break;
            }
        }
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.state.lock().map(|state| *state).unwrap_or(ActorState::Idle)
    }

    fn wakeup(&self) {
        self.woken.store(true, Ordering::SeqCst);
    }

    fn cancel(&self) {
        self.ask(Interrupt::Cancel);
    }

    fn turn_under_way(&self) -> Option<TurnId> {
        self.turn.lock().ok().and_then(|turn| *turn)
    }

    fn retry_turn(&self, turn: TurnId) -> bool {
        let under_way = self.turn_under_way() == Some(turn);
        if under_way {
            self.ask(Interrupt::Retry);
        }
        under_way
    }

    fn stop(&self) {
        self.ask(Interrupt::Stop);
    }
}
