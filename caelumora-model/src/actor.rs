use crate::agent::{AgentId, RetryRefused};
use crate::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApproverId};
use crate::channel::ChannelId;
use crate::confidentiality::Classification;
use crate::message::{Message, MessageId};
use crate::question::{QuestionId, QuestionRequest, Response};
use crate::security::SecurityContext;
use crate::session::SessionId;
use crate::user::UserId;
use crate::turn::GivenUpReason;
use crate::value::name::Name;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorId {
    #[default]
    None,
    User(UserId),
    Agent(AgentId),
    Approver(ApproverId),
    /// The runtime itself, acting on no one's behalf: setting up initial
    /// data, keeping its own records in step with events. Only ever set
    /// deliberately, through `SecurityContext::system()` - never the default,
    /// so that a context nobody filled in has no rights (issue 11).
    System,
}
impl ActorId {
    /// The name every approver is shown with.
    pub const APPROVER_NAME: &'static str = "Approver";

    /// The name every approver is shown with, as a name: what an approver's
    /// actor is called in its channel.
    pub fn approver_name() -> Name {
        Name::fixed(Self::APPROVER_NAME)
    }

    /// The name of an actor that is no entity and so has no name of its own:
    /// an approver, the runtime itself, nobody. `None` for users and agents,
    /// whose names are their entities' (`ControllerManager::actor_name`).
    pub fn fixed_name(&self) -> Option<&'static str> {
        match self {
            ActorId::Approver(_) => Some(Self::APPROVER_NAME),
            ActorId::System => Some("system"),
            ActorId::None => Some("unknown"),
            ActorId::User(_) | ActorId::Agent(_) => None,
        }
    }

    pub fn id(&self) -> Uuid {
        match &self {
            ActorId::None => {Uuid::nil()}
            ActorId::User(id) => {id.0}
            ActorId::Agent(id) => {id.0}
            ActorId::Approver(id) => {id.0}
            ActorId::System => {Uuid::nil()}
        }
    }

    pub fn as_agent_id(&self) -> Option<AgentId> {
        match self {
            ActorId::Agent(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_user_id(&self) -> Option<UserId> {
        match self {
            ActorId::User(id) => Some(*id),
            _ => None,
        }
    }
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorEvent {
    ApprovalRequested(ApprovalRequest),
    ApprovalGranted(ApprovalResponse),
    ApprovalDenied(ApprovalResponse),
    ApprovalTimedOut(ApprovalResponse),
    /// The agent no longer waits for the decision on its request `id`: the
    /// turn that made the call ended first (issue 148). Sent by the agent to
    /// the approvers, which take it only from the agent that asked, and by
    /// an approver to the users, so that nobody is asked any longer.
    ApprovalRequestWithdrawn { id: ApprovalId },
    /// An agent's call asks its questions (issue 165): sent by the agent to
    /// the users - to those asked alone if the questions are sensitive.
    QuestionRequested(QuestionRequest),
    /// A user's answers to the questions `id` of agent `agent_id`, sent to
    /// that agent alone; who gave them is the envelope's sender.
    QuestionAnswered { id: QuestionId, agent_id: AgentId, responses: Vec<Response> },
    /// A user declines to answer the questions `id` of agent `agent_id` -
    /// by saying so, or by writing a message instead - sent to that agent
    /// alone.
    QuestionDeclined { id: QuestionId, agent_id: AgentId },
    /// The agent no longer waits for the answers to its questions `id`:
    /// they were answered or declined, or its turn ended first. Sent to the
    /// users it asked them of - `shown_to`, or every user - so that their
    /// frontends no longer offer to answer.
    QuestionClosed { id: QuestionId, shown_to: Option<Vec<ActorId>> },
    ResponseRequested,
    /// An answer begins: the turn `turn_id` of the sender (issue 235) - an
    /// agent's turn or an external agent's work - whose messages carry the
    /// same id. Announced before anything of it, so that an answer that is
    /// not streamed is known as under way, and which turn it is, before it
    /// ends.
    ResponseStarted { turn_id: TurnId },
    /// The sender now stands at this state (issue 243): announced by an
    /// agent as it changes, so that the users see who works, sleeps or
    /// waits for an approval. Sent in the agent's own contexts, not a
    /// turn's: the state is the agent's, not part of an answer.
    StateChanged(ActorState),
    PartialMessageGenerated(Message),
    CompleteMessageGenerated(Message),
    /// The turn `turn_id` ended and was taken over, with its answer,
    /// `messages` - those that are not internal (issue 258). The channel
    /// records the turn's end from it, and the moderator decides from it
    /// whom to ask to respond; the messages themselves were said before,
    /// each as a `CompleteMessageGenerated`, which is what the channel keeps
    /// and the agents hear. A message said outside of any turn - a user's,
    /// one an external agent says unasked - is answered by itself: it has no
    /// turn to end.
    TurnFinished { turn_id: TurnId, messages: Vec<Message> },
    WakeupRequested,
    /// Streaming text produced by the agent while it is not yet ready to give
    /// its final answer - still deliberating (researching, proposing a plan)
    /// or executing an already-approved plan. Never the final response.
    IntermediatePartialGenerated(Message),
    /// A completed message that still carries tool calls (or proposes a
    /// plan), produced while deliberating or executing an approved plan -
    /// not the final answer.
    IntermediateMessageGenerated(Message),
    /// A plan step of the turn `turn_id` has just begun execution. Emitted
    /// directly by `AgentActor` whenever it pushes that step's instruction,
    /// so observers (e.g. the web UI) can show which step of the plan is in
    /// progress; the channel records it in its history (issue 263).
    PlanStepStarted {
        turn_id: TurnId,
        step_index: usize,
        total_steps: usize,
        description: String,
    },
    /// The plan step that was in progress has converged. Emitted directly by
    /// `AgentActor` right before it advances to the next step (or to
    /// finalization).
    PlanStepFinished {
        turn_id: TurnId,
        step_index: usize,
        total_steps: usize,
        description: String,
    },
    /// The agent's turn `turn_id` was given up for `reason` (issues 145,
    /// 258): it failed - an unreachable LLM backend, a failing component -,
    /// the channel was lowered below it, or its answer was asked for again.
    /// The agent is idle again and takes the next request. The message of
    /// a failure is meant for users: it names what failed (a component, an
    /// endpoint) but carries no file paths or secret values; the full error
    /// is in the log under the envelope's `span_id`. A stopped agent sends
    /// none: it has left the channel.
    ///
    /// Shown to users; not heard by the agents.
    TurnGivenUp {
        turn_id: TurnId,
        reason: GivenUpReason,
    },
    /// A user asks the agent `agent` to carry out the command `command`
    /// with `args` (issue 269): sent by the user's actor, which checked that
    /// the issuer may, and routed to `agent` alone, which checks again and
    /// carries it out in a turn of its own. The envelope's sender is the
    /// issuer, its security context theirs. It wakes nobody: an agent
    /// carries a command out asleep, and a quiet channel delivers it.
    CommandRequested {
        command: String,
        args: String,
        agent: ActorId,
    },
    /// A command was carried out (issues 28, 269): `command` is its name
    /// without the slash, `args` what it was given, `output` what it
    /// answered. One of the session - `/wakeup` - the envelope's sender is
    /// the user who ran it, sent only when the answer concerns everybody;
    /// one an agent carried out, the sender is the agent, `turn_id` the
    /// turn it was carried out in, and `to` the issuer alone if the answer
    /// is theirs, `None` if it is everybody's - the issuer is the security
    /// context's initiator either way.
    ///
    /// Shown to users, never heard by agents. A command of the session is
    /// never written to the history - it addresses the system -; one an
    /// agent carried out is recorded by the channel, without the output
    /// (`HistoryEntry::CommandExecuted`).
    CommandExecuted {
        command: String,
        args: String,
        output: String,
        to: Option<ActorId>,
        turn_id: Option<TurnId>,
    },
    /// The agent did not carry out the command `command` (issue 269): it
    /// is none of its own, the issuer may not run it, its arguments are
    /// invalid, or it failed. `reason` is worded for the issuer, `to`, as
    /// `CommandError` words it - never the error itself, which stays in
    /// the log. Shown to the issuer alone and never written to a history:
    /// nothing happened.
    CommandFailed {
        command: String,
        reason: String,
        to: ActorId,
    },
    /// `participant` was removed from the channel (issue 162). The envelope's
    /// sender is who removed it - the runtime itself, `ActorId::System`, if
    /// nobody did. Sent by the channel once the participant's actor no longer
    /// runs in it: nothing of the participant follows, so whatever of it was
    /// under way - a turn, a call of a tool - has ended.
    ///
    /// Shown to users and never written to the history.
    ParticipantRemoved {
        participant: ActorId,
    },
    /// A chain between agents reached its limit (issue 200): the response
    /// ending in `message` was delivered, but it made nobody respond, since
    /// that would have been more than `max_agent_hops` hops since the last
    /// user's message. `not_asked` are the agents it mentioned, empty if it
    /// mentioned none. Sent by the moderator in the name of
    /// `ActorId::System`, to the users of the channel and to the agents
    /// involved: the sender of the response and those it mentioned.
    ///
    /// Never written to the history; an agent takes no note of it.
    ChainCutOff {
        message: MessageId,
        max_agent_hops: u32,
        not_asked: Vec<ActorId>,
    },
    /// The message `0.id` reads as `0` from now on (issue 234): sent by its
    /// author alone, with the id, sender and role of the message it
    /// modifies and the time it was modified as its `timestamp`. The channel
    /// admits it only for a message of its history that its sender wrote
    /// and that was not taken back, records it in its history, and every
    /// participant that holds the message records it too
    /// (`Message::modify`). It asks nobody to respond.
    MessageModified(Message),
    /// The message `message` was taken back by its author at `at` (issue
    /// 234), sent and admitted as `MessageModified`: recorded wherever the
    /// message is held (`Message::revoke`), never removed. It asks nobody to
    /// respond.
    MessageRevoked {
        message: MessageId,
        at: DateTime<Utc>,
    },
    /// The message `message` is classified `classification` from now on
    /// (issues 133, 239): sent by its author or the channel's owner, without
    /// content. The channel admits it only for a message of its history,
    /// not taken back, that its sender may reclassify so - never above the
    /// channel - and it is recorded wherever the message is held
    /// (`Message::reclassify`). It asks nobody to respond.
    MessageReclassified {
        message: MessageId,
        classification: Classification,
    },
    /// `author`, an agent, is asked to answer again instead of `target`
    /// (issues 234, 235): sent by a user's actor and routed to `author`
    /// alone. A message is admitted by the channel only if it is one of its
    /// history that `author` said, and whether the agent can is its own to
    /// decide, by its own transcript. A turn is asked of the answer under
    /// way: the channel ends that turn before it delivers the request - if
    /// it is still the one under way, else it drops the request - and the
    /// agent answers anew. It answers `RetryAccepted` or `RetryRejected`.
    RetryRequested {
        target: RetryTarget,
        author: ActorId,
    },
    /// The agent answers again instead of `target` (issues 234, 235): it
    /// takes back what it replaces (`MessageRevoked`) - nothing of a turn
    /// that was under way, which it gave up - and answers in a turn of its
    /// own. Shown to users and never written to the history.
    RetryAccepted {
        target: RetryTarget,
    },
    /// The agent does not answer again instead of `target`, for `reason`
    /// (issue 234) - one of a known few, which a user interface words
    /// itself. Shown to users and never written to the history.
    RetryRejected {
        target: RetryTarget,
        reason: RetryRefused,
    },
}

/// What a request to answer again is about (issue 235).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryTarget {
    /// A message the agent said, in the channel's history (issue 234).
    Message(MessageId),
    /// The answer the agent gives in this turn, still under way.
    Turn(TurnId),
}

impl std::fmt::Display for RetryTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryTarget::Message(message) => write!(f, "message {message}"),
            RetryTarget::Turn(turn) => write!(f, "turn {turn}"),
        }
    }
}

impl ActorEvent {
    /// The event as it leaves its sender for the channel (issues 54, 133):
    /// the messages it carries without what only their owner sees
    /// (`Message::published`). `None` if it carried a single message of which
    /// nothing is left to show. Approval requests are left as they are: the
    /// approvers see the arguments of a call they are asked to decide.
    pub fn published(self) -> Option<ActorEvent> {
        let publish = |message: Message, wrap: fn(Message) -> ActorEvent| message.published().map(wrap);
        match self {
            ActorEvent::PartialMessageGenerated(m) => publish(m, ActorEvent::PartialMessageGenerated),
            ActorEvent::CompleteMessageGenerated(m) => publish(m, ActorEvent::CompleteMessageGenerated),
            ActorEvent::IntermediatePartialGenerated(m) => publish(m, ActorEvent::IntermediatePartialGenerated),
            ActorEvent::IntermediateMessageGenerated(m) => publish(m, ActorEvent::IntermediateMessageGenerated),
            ActorEvent::MessageModified(m) => publish(m, ActorEvent::MessageModified),
            ActorEvent::TurnFinished { turn_id, messages } => Some(ActorEvent::TurnFinished {
                turn_id,
                messages: messages.iter().filter_map(Message::published).collect(),
            }),
            other => Some(other),
        }
    }

    /// The messages the event carries, in order; none for an event that
    /// carries no message.
    pub fn messages(&self) -> Vec<&Message> {
        match self {
            ActorEvent::PartialMessageGenerated(m)
            | ActorEvent::CompleteMessageGenerated(m)
            | ActorEvent::IntermediatePartialGenerated(m)
            | ActorEvent::IntermediateMessageGenerated(m)
            | ActorEvent::MessageModified(m) => vec![m],
            ActorEvent::TurnFinished { messages, .. } => messages.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// The variant's name, for logs - never the payload, which may carry
    /// message content.
    pub fn kind(&self) -> &'static str {
        match self {
            ActorEvent::ApprovalRequested(_) => "ApprovalRequested",
            ActorEvent::ApprovalGranted(_) => "ApprovalGranted",
            ActorEvent::ApprovalDenied(_) => "ApprovalDenied",
            ActorEvent::ApprovalTimedOut(_) => "ApprovalTimedOut",
            ActorEvent::ApprovalRequestWithdrawn { .. } => "ApprovalRequestWithdrawn",
            ActorEvent::QuestionRequested(_) => "QuestionRequested",
            ActorEvent::QuestionAnswered { .. } => "QuestionAnswered",
            ActorEvent::QuestionDeclined { .. } => "QuestionDeclined",
            ActorEvent::QuestionClosed { .. } => "QuestionClosed",
            ActorEvent::ResponseRequested => "ResponseRequested",
            ActorEvent::ResponseStarted { .. } => "ResponseStarted",
            ActorEvent::StateChanged(_) => "StateChanged",
            ActorEvent::PartialMessageGenerated(_) => "PartialMessageGenerated",
            ActorEvent::CompleteMessageGenerated(_) => "CompleteMessageGenerated",
            ActorEvent::TurnFinished { .. } => "TurnFinished",
            ActorEvent::WakeupRequested => "WakeupRequested",
            ActorEvent::IntermediatePartialGenerated(_) => "IntermediatePartialGenerated",
            ActorEvent::IntermediateMessageGenerated(_) => "IntermediateMessageGenerated",
            ActorEvent::PlanStepStarted { .. } => "PlanStepStarted",
            ActorEvent::PlanStepFinished { .. } => "PlanStepFinished",
            ActorEvent::TurnGivenUp { .. } => "TurnGivenUp",
            ActorEvent::CommandRequested { .. } => "CommandRequested",
            ActorEvent::CommandExecuted { .. } => "CommandExecuted",
            ActorEvent::CommandFailed { .. } => "CommandFailed",
            ActorEvent::ParticipantRemoved { .. } => "ParticipantRemoved",
            ActorEvent::ChainCutOff { .. } => "ChainCutOff",
            ActorEvent::MessageModified(_) => "MessageModified",
            ActorEvent::MessageRevoked { .. } => "MessageRevoked",
            ActorEvent::MessageReclassified { .. } => "MessageReclassified",
            ActorEvent::RetryRequested { .. } => "RetryRequested",
            ActorEvent::RetryAccepted { .. } => "RetryAccepted",
            ActorEvent::RetryRejected { .. } => "RetryRejected",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub event_id: Uuid,
    pub correlation_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
    /// How many times since the last message of a user an agent's message
    /// made another agent respond (issue 45). A user's message starts at 0;
    /// `next` keeps it, and the moderator counts a hop when it asks an agent
    /// to respond to an agent. Absent in what was stored before it existed.
    #[serde(default)]
    pub agent_hops: u32,
}

impl EventContext {
    pub fn new(session_id: SessionId, channel_id: ChannelId, sender_id:ActorId, sender_name: impl Into<String>) -> Self {
        Self {
            session_id,
            channel_id,
            sender_id,
            sender_name: sender_name.into(),
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::default(),
            span_id: Uuid::new_v4(),
            span_depth: 0,
            agent_hops: 0,
        }
    }

    pub fn next(&self, sender_id:ActorId, sender_name: impl Into<String>) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            sender_id,
            sender_name: sender_name.into(),
            event_id: Uuid::new_v4(),
            correlation_id: self.event_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
            agent_hops: self.agent_hops,
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub timestamp: DateTime<Utc>,
    pub security: SecurityContext,
    pub context: EventContext,
    pub event: ActorEvent
}


impl Envelope {
    pub fn new(security: SecurityContext, context: EventContext, event: ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security,
            context,
            event
        }
    }

    /// The same envelope - sender, contexts - with `event` instead, e.g. a
    /// moderator passing an answer on.
    pub fn forward(&self, event: ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event
        }
    }

    /// Short, human-readable summary of this envelope's event, used for logging.
    pub fn describe(&self) -> String {
        let ctx = &self.context;
        match &self.event {
            ActorEvent::PartialMessageGenerated(m) => {
                format!("Partial(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
            }
            ActorEvent::CompleteMessageGenerated(m) => {
                format!("Complete(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
            }
            ActorEvent::TurnFinished { messages: v, .. } => match v.last() {
                Some(m) => format!("EndOfTurn(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id),
                None => format!("EndOfTurn(sender_id={}, sender_name={}, no message)", ctx.sender_id, ctx.sender_name),
            },
            ActorEvent::ResponseRequested => {
                format!("ResponseRequested(sender_id={}, sender_name={}", ctx.sender_id, ctx.sender_name)
            }
            ActorEvent::IntermediatePartialGenerated(m) => {
                format!("IntermediatePartial(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
            }
            ActorEvent::IntermediateMessageGenerated(m) => {
                format!("IntermediateMessage(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
            }
            ActorEvent::PlanStepStarted { step_index, total_steps, .. } => {
                format!("PlanStepStarted(sender_id={}, sender_name={}, step={}/{})", ctx.sender_id, ctx.sender_name, step_index + 1, total_steps)
            }
            ActorEvent::PlanStepFinished { step_index, total_steps, .. } => {
                format!("PlanStepFinished(sender_id={}, sender_name={}, step={}/{})", ctx.sender_id, ctx.sender_name, step_index + 1, total_steps)
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                format!("TurnGivenUp(sender_id={}, sender_name={}, turn_id={}, reason={})", ctx.sender_id, ctx.sender_name, turn_id, reason.cause())
            }
            ActorEvent::CommandRequested { command, agent, .. } => {
                format!("CommandRequested(sender_id={}, sender_name={}, command={}, agent={})", ctx.sender_id, ctx.sender_name, command, agent)
            }
            ActorEvent::CommandExecuted { command, to, .. } => {
                format!("CommandExecuted(sender_id={}, sender_name={}, command={}, to={})", ctx.sender_id, ctx.sender_name, command, to.map_or("everybody".to_string(), |to| to.to_string()))
            }
            ActorEvent::CommandFailed { command, to, .. } => {
                format!("CommandFailed(sender_id={}, sender_name={}, command={}, to={})", ctx.sender_id, ctx.sender_name, command, to)
            }
            ActorEvent::ParticipantRemoved { participant } => {
                format!("ParticipantRemoved(sender_id={}, sender_name={}, participant={})", ctx.sender_id, ctx.sender_name, participant)
            }
            ActorEvent::ChainCutOff { message, max_agent_hops, not_asked } => {
                format!("ChainCutOff(sender_id={}, message={}, max_agent_hops={}, not_asked={})", ctx.sender_id, message, max_agent_hops, not_asked.len())
            }
            ActorEvent::MessageModified(m) => {
                format!("MessageModified(sender_id={}, sender_name={}, mid={})", ctx.sender_id, ctx.sender_name, m.id)
            }
            ActorEvent::MessageRevoked { message, .. } => {
                format!("MessageRevoked(sender_id={}, sender_name={}, mid={})", ctx.sender_id, ctx.sender_name, message)
            }
            ActorEvent::MessageReclassified { message, classification } => {
                format!("MessageReclassified(sender_id={}, sender_name={}, mid={}, classification={})", ctx.sender_id, ctx.sender_name, message, classification)
            }
            ActorEvent::RetryRequested { target, author } => {
                format!("RetryRequested(sender_id={}, {}, author={})", ctx.sender_id, target, author)
            }
            ActorEvent::RetryAccepted { target } => {
                format!("RetryAccepted(sender_id={}, {})", ctx.sender_id, target)
            }
            ActorEvent::RetryRejected { target, reason } => {
                format!("RetryRejected(sender_id={}, {}, reason={:?})", ctx.sender_id, target, reason)
            }
            _ => {
                format!("{:?}", self.event)
            }
        }
    }
}


#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActorState {
    #[default]
    Idle,
    /// Calling the LLM and streaming its response - covers everything from
    /// initial research/deliberation through proposing or extending a plan
    /// to executing its steps, and the ordinary single-shot tool loop for
    /// personalities without extended planning.
    Planning,
    /// Running approved tool calls.
    ExecutingTools,
    /// Persisting context/history and closing out the turn.
    Reflecting,
    Cancelling,
    /// Fall asleep, do not generate new messages
    Sleeping,
    /// Waiting for a person: an approval of a tool call (issue 165).
    WaitingForUser,
}

impl ActorState {
    /// Every state, in the order of its declaration - its byte
    /// (`#[repr(u8)]`) is its place here.
    const ALL: [ActorState; 7] = [
        ActorState::Idle,
        ActorState::Planning,
        ActorState::ExecutingTools,
        ActorState::Reflecting,
        ActorState::Cancelling,
        ActorState::Sleeping,
        ActorState::WaitingForUser,
    ];

    /// Whether the state belongs to a turn - an error in it ends the turn,
    /// not the actor - rather than waiting between turns.
    pub fn in_turn(self) -> bool {
        !matches!(self, ActorState::Idle | ActorState::Sleeping)
    }

    /// Whether a cancel can still end the turn in this state (issues 235,
    /// 243): not one ending already, nor none.
    pub fn interruptible(self) -> bool {
        matches!(self, ActorState::Planning | ActorState::ExecutingTools | ActorState::WaitingForUser)
    }

    fn from_raw(v: u8) -> Self {
        Self::ALL.get(usize::from(v)).copied().unwrap_or_default()
    }

    fn raw(self) -> u8 {
        self as u8
    }
}

/// The live state of an actor, read by other tasks - the moderator, the
/// channel - while the actor's run loop moves it on.
#[derive(Debug, Default)]
pub struct ProcessingStateHolder {
    state: AtomicU8
}

impl ProcessingStateHolder {
    pub fn new(state: ActorState) -> Self {
        Self {
            state: AtomicU8::new(state.raw())
        }
    }

    pub fn load(&self) -> ActorState {
        ActorState::from_raw(self.state.load(Ordering::SeqCst))
    }

    pub fn store(&self, state: ActorState) {
        self.state.store(state.raw(), Ordering::SeqCst);
    }

    /// Sets the state to `state` and returns the one before.
    pub fn swap(&self, state: ActorState) -> ActorState {
        ActorState::from_raw(self.state.swap(state.raw(), Ordering::SeqCst))
    }

    /// Moves the state from `from` to `to` if it is `from`; whether it was.
    pub fn replace(&self, from: ActorState, to: ActorState) -> bool {
        self.state.compare_exchange(from.raw(), to.raw(), Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }
}




#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorState;

    #[test]
    fn the_approvers_name_is_a_name() {
        assert_eq!(Name::new(ActorId::APPROVER_NAME).map(|name| name.as_str().to_string()), Ok(ActorId::approver_name().as_str().to_string()));
        assert_eq!(ActorId::approver_name(), "approver");
    }

    #[test]
    fn every_state_is_kept_as_its_own_byte() {
        for state in ActorState::ALL {
            assert_eq!(ActorState::from_raw(state.raw()), state);
        }
        let holder = ProcessingStateHolder::new(ActorState::Sleeping);
        assert!(!holder.replace(ActorState::Planning, ActorState::Idle), "not in that state");
        assert!(holder.replace(ActorState::Sleeping, ActorState::Idle));
        assert_eq!(holder.load(), ActorState::Idle);
        assert!(ActorState::WaitingForUser.in_turn() && !ActorState::Sleeping.in_turn());
    }

    #[test]
    fn published_events_carry_redacted_messages() {
        use crate::tool::{ToolCall, ToolVisibility};
        let hidden = Message {
            tool_calls: vec![ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() }],
            ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let text = Message { content: Some("hi".to_string()), ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT) };

        assert!(ActorEvent::CompleteMessageGenerated(hidden.clone()).published().is_none(), "nothing left to show");
        assert!(ActorEvent::PartialMessageGenerated(hidden.clone()).published().is_none());
        match (ActorEvent::TurnFinished { turn_id: crate::work::TurnId::new(), messages: vec![hidden.clone(), text.clone()] }).published() {
            Some(ActorEvent::TurnFinished { messages, .. }) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].id, text.id);
            }
            other => panic!("expected the response, got {other:?}"),
        }
        assert!(
            matches!(ActorEvent::TurnFinished { turn_id: crate::work::TurnId::new(), messages: vec![hidden] }.published(), Some(ActorEvent::TurnFinished { messages: m, .. }) if m.is_empty()),
            "the end of the turn is still announced"
        );
    }
}
