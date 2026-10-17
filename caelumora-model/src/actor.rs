use crate::agent::{AgentId, RetryRefused};
use crate::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApproverId};
use crate::channel::ChannelId;
use crate::confidentiality::Classification;
use crate::history::HistoryEntry;
use crate::message::{Message, MessageId};
use crate::question::{QuestionId, QuestionRequest, Response};
use crate::security::SecurityContext;
use crate::session::SessionId;
use crate::user::UserId;
use crate::turn::GivenUpReason;
use crate::mention::handle_of;
use crate::value::name::Name;
use crate::value::text::Introduction;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use uuid::Uuid;
use crate::store::query::FieldValue;
use std::fmt;

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
/// An actor as a query compares it, after serde's tags: `{"User": "…"}`,
/// `"System"`. A participant, whose variants have the same form, is asked
/// by the actor it is.
impl From<ActorId> for FieldValue {
    fn from(actor: ActorId) -> Self {
        match actor {
            ActorId::None => FieldValue::from("None"),
            ActorId::User(id) => FieldValue::tagged("User", id),
            ActorId::Agent(id) => FieldValue::tagged("Agent", id),
            ActorId::Approver(id) => FieldValue::tagged("Approver", id),
            ActorId::System => FieldValue::from("System"),
        }
    }
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
    /// A user asks `participant` - an agent, another user, any actor of the
    /// channel - to respond to the conversation as it stands (issue 278),
    /// without saying anything: sent by the user's actor, which the
    /// moderator turns into the `ResponseRequested` to `participant` alone -
    /// only the moderator asks for a response. The envelope's sender is the
    /// user, its security context theirs, so an agent's turn acts with their
    /// authority, as after a message of theirs; a user is shown the request.
    /// It is deliberate, as a `/wakeup` is: the channel ends being quiet,
    /// and an agent is woken as anyone the moderator asks is. Never written
    /// to the history, never shown itself: what remains of it is the answer.
    NudgeRequested { participant: ActorId },
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
    /// The agent began to carry out the command `command` with `args`,
    /// asked by `to` (issue 299), in the turn `turn_id`: it took it up and
    /// may ask for it. Told to the issuer alone, whatever its answer's
    /// visibility - the answer, `CommandExecuted` or `CommandFailed` with
    /// the same `turn_id`, ends it. Never written to a history.
    CommandStarted {
        command: String,
        args: String,
        to: ActorId,
        turn_id: TurnId,
    },
    /// The agent did not carry out the command `command` (issue 269): it
    /// is none of its own, the issuer may not run it, its arguments are
    /// invalid, or it failed. `reason` is worded for the issuer, `to`, as
    /// `CommandError` words it - never the error itself, which stays in
    /// the log. `turn_id` is the turn it had begun in, if it told the
    /// issuer so (`CommandStarted`). Shown to the issuer alone and never
    /// written to a history: nothing happened.
    CommandFailed {
        command: String,
        reason: String,
        to: ActorId,
        turn_id: Option<TurnId>,
    },
    /// The agent began `activity` of its own accord in its turn `turn_id`
    /// (issue 284) - something besides answering, which nobody asked for,
    /// as against a command: "compacting its context". Worded for users, as
    /// a command answers; its end is `ActivityFinished` with the same
    /// `turn_id`, or the turn's being given up (`TurnGivenUp`). Shown to
    /// users, never heard by agents, never written to a history.
    ActivityStarted {
        turn_id: TurnId,
        activity: String,
    },
    /// The agent's `activity` in its turn `turn_id` ended (issue 284), and
    /// `output` says what came of it, worded for users. Shown to users,
    /// never heard by agents; the channel records it in its history
    /// (`HistoryEntry::ActivityFinished`), so that it is shown where it
    /// happened.
    ActivityFinished {
        turn_id: TurnId,
        activity: String,
        output: String,
    },
    /// `participant`, called `name`, joined the channel (issue 408). The
    /// envelope's sender is who added it - the participant itself if it
    /// joined by itself, the runtime, `ActorId::System`, if an agent was
    /// restored. Sent by the channel alone, as the participant's actor
    /// starts, so that it is the first thing the newcomer hears.
    ///
    /// The channel records it in its history (`HistoryEntry::
    /// ParticipantJoined`), and everybody is told - the agents too, which
    /// take note of it without responding -, also whose name it is told in.
    ParticipantJoined {
        participant: ActorId,
        name: String,
    },
    /// `participant`, called `name`, left the channel (issues 162, 408). The
    /// envelope's sender is who removed it - the runtime itself,
    /// `ActorId::System`, if nobody did. Sent by the channel alone, once the
    /// participant's actor no longer runs in it: nothing of the participant
    /// follows, so whatever of it was under way - a turn, a call of a tool -
    /// has ended.
    ///
    /// Recorded and told as `ParticipantJoined` is.
    ParticipantLeft {
        participant: ActorId,
        name: String,
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
    /// The channel did not let in an event of the receiver's own (issue
    /// 304): `rejected` names its kind (`ActorEvent::kind`), `message` the
    /// message it was or was about, if any, and `reason` is one of a known
    /// few, which a user interface words itself - the channel's one decision
    /// of what enters it, made in order as it takes the event in. Sent by the
    /// channel to the event's sender alone, and never written to the
    /// history: nothing entered it.
    MessageRejected {
        rejected: String,
        message: Option<MessageId>,
        reason: Rejection,
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

impl fmt::Display for RetryTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

    /// The message the event is or is about, if any: a message said, or one
    /// it changes, takes back, reclassifies or asks to be answered again -
    /// what a rejection of it names (`MessageRejected`).
    pub fn subject_message(&self) -> Option<MessageId> {
        match self {
            ActorEvent::PartialMessageGenerated(m)
            | ActorEvent::CompleteMessageGenerated(m)
            | ActorEvent::IntermediatePartialGenerated(m)
            | ActorEvent::IntermediateMessageGenerated(m)
            | ActorEvent::MessageModified(m) => Some(m.id),
            ActorEvent::MessageRevoked { message, .. } | ActorEvent::MessageReclassified { message, .. } => Some(*message),
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), .. } => Some(*message),
            _ => None,
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

    /// Whether it is the channel's own word, which no actor sends: who
    /// joined or left (issue 408). It reaches whose name it is told in,
    /// too - nothing of theirs is echoed back to them.
    pub fn is_announcement(&self) -> bool {
        matches!(self, ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. })
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
            ActorEvent::NudgeRequested { .. } => "NudgeRequested",
            ActorEvent::CommandStarted { .. } => "CommandStarted",
            ActorEvent::CommandExecuted { .. } => "CommandExecuted",
            ActorEvent::CommandFailed { .. } => "CommandFailed",
            ActorEvent::ActivityStarted { .. } => "ActivityStarted",
            ActorEvent::ActivityFinished { .. } => "ActivityFinished",
            ActorEvent::ParticipantJoined { .. } => "ParticipantJoined",
            ActorEvent::ParticipantLeft { .. } => "ParticipantLeft",
            ActorEvent::ChainCutOff { .. } => "ChainCutOff",
            ActorEvent::MessageModified(_) => "MessageModified",
            ActorEvent::MessageRevoked { .. } => "MessageRevoked",
            ActorEvent::MessageReclassified { .. } => "MessageReclassified",
            ActorEvent::RetryRequested { .. } => "RetryRequested",
            ActorEvent::RetryAccepted { .. } => "RetryAccepted",
            ActorEvent::RetryRejected { .. } => "RetryRejected",
            ActorEvent::MessageRejected { .. } => "MessageRejected",
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


/// Why the channel did not let an event in (`ActorEvent::MessageRejected`,
/// issue 304), worded for its sender, as the web UI and the TUI word it
/// after "… was not let in:".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    /// Its sender may not do this here - change another's message, ask for
    /// another answer, answer questions in this channel.
    #[error("you may not do this here")]
    Forbidden,
    /// Classified so, it may not enter the channel - above it, or with a
    /// compartment it does not admit or that is not configured.
    #[error("it is classified so that the channel does not admit it")]
    ClassificationRefused,
    /// The message it is about is not in the channel.
    #[error("the message it is about is not in the channel")]
    MessageNotFound,
    /// The message it is about was taken back, or cannot be changed so.
    #[error("the message it is about was taken back or cannot be changed so")]
    MessageUnchangeable,
    /// It does not fit the channel as it is: sent in another's name, about
    /// someone who takes no part, or asking what cannot be asked.
    #[error("it does not fit the channel as it is")]
    Invalid,
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

    /// What the channel's history records of this envelope if it is an
    /// announcement of the channel's own (`ActorEvent::is_announcement`,
    /// issue 408): who joined or left, by its sender, when it was sent.
    /// `None` for any other event.
    pub fn announced(&self) -> Option<HistoryEntry> {
        self.entry().filter(|_| self.event.is_announcement())
    }

    /// What a channel's history records of this envelope (issue 417): a
    /// message said, a step of it included (issue 258); how a turn ended;
    /// where a step of its plan began (issue 263); a command an agent
    /// carried out (issue 269) - who asked is the initiator; what came of
    /// what an agent did of its own accord (issue 284); who joined or left,
    /// by its sender (issue 408); a change of a message (issue 234). Every
    /// mark in the name of its sender, when it was sent. `None` for what the
    /// history does not record: what streams, a beginning whose end is
    /// recorded, where an actor stands, what only asks, approvals and
    /// questions, a command of the session.
    pub fn entry(&self) -> Option<HistoryEntry> {
        let actor = self.context.sender_id;
        let at = self.timestamp;
        Some(match &self.event {
            ActorEvent::CompleteMessageGenerated(message) | ActorEvent::IntermediateMessageGenerated(message) => HistoryEntry::Said { message: message.clone(), context: None },
            ActorEvent::TurnFinished { turn_id, .. } => HistoryEntry::TurnFinished { turn_id: *turn_id, actor, at },
            ActorEvent::TurnGivenUp { turn_id, reason } => HistoryEntry::TurnGivenUp { turn_id: *turn_id, actor, reason: reason.clone(), at },
            ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description } => {
                HistoryEntry::PlanStepStarted { turn_id: *turn_id, actor, step_index: *step_index, total_steps: *total_steps, description: description.clone(), at }
            }
            ActorEvent::CommandExecuted { turn_id: Some(turn_id), command, args, .. } => {
                HistoryEntry::CommandExecuted { turn_id: *turn_id, actor, issuer: self.security.initiator_id, command: command.clone(), args: args.clone(), at }
            }
            ActorEvent::ActivityFinished { turn_id, activity, output } => HistoryEntry::ActivityFinished { turn_id: *turn_id, actor, activity: activity.clone(), output: output.clone(), at },
            ActorEvent::ParticipantJoined { participant, name } => {
                HistoryEntry::ParticipantJoined { participant: *participant, name: name.clone(), by: actor, by_name: self.context.sender_name.clone(), at }
            }
            ActorEvent::ParticipantLeft { participant, name } => {
                HistoryEntry::ParticipantLeft { participant: *participant, name: name.clone(), by: actor, by_name: self.context.sender_name.clone(), at }
            }
            ActorEvent::MessageModified(modified) => HistoryEntry::Modified { message: modified.clone() },
            ActorEvent::MessageRevoked { message, at } => HistoryEntry::Revoked { message: *message, at: *at },
            ActorEvent::MessageReclassified { message, classification } => HistoryEntry::Reclassified { message: *message, classification: classification.clone() },
            ActorEvent::ApprovalRequested(_) | ActorEvent::ApprovalGranted(_) | ActorEvent::ApprovalDenied(_) | ActorEvent::ApprovalTimedOut(_)
            | ActorEvent::ApprovalRequestWithdrawn { .. } | ActorEvent::QuestionRequested(_) | ActorEvent::QuestionAnswered { .. }
            | ActorEvent::QuestionDeclined { .. } | ActorEvent::QuestionClosed { .. } | ActorEvent::ResponseRequested | ActorEvent::ResponseStarted { .. }
            | ActorEvent::StateChanged(_) | ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) | ActorEvent::WakeupRequested
            | ActorEvent::PlanStepFinished { .. } | ActorEvent::CommandRequested { .. } | ActorEvent::NudgeRequested { .. } | ActorEvent::CommandExecuted { turn_id: None, .. }
            | ActorEvent::CommandStarted { .. } | ActorEvent::CommandFailed { .. } | ActorEvent::ActivityStarted { .. } | ActorEvent::ChainCutOff { .. }
            | ActorEvent::RetryRequested { .. } | ActorEvent::RetryAccepted { .. } | ActorEvent::RetryRejected { .. } | ActorEvent::MessageRejected { .. } => return None,
        })
    }

    /// The envelope as everybody but its sender may see it (issues 54, 133,
    /// 417): its messages without what only their owner sees
    /// (`ActorEvent::published`); `None` if it carried a single message of
    /// which nothing is left to show. What a channel gives its moderator and
    /// delivers.
    pub fn published(&self) -> Option<Envelope> {
        self.event.clone().published().map(|event| Envelope { event, ..self.clone() })
    }

    /// Whether the envelope concerns `participant` by the rule every channel
    /// delivers by (issue 418): whom it goes to unless a moderator decides
    /// otherwise. A message to every user, and to every agent if there is
    /// something to hear in it (`Message::is_heard`, issues 224, 258); a
    /// change of a message to every user and agent (issue 234); who joined
    /// or left to every user and agent (issue 408); how an answer comes
    /// about, how a turn ended, where an agent stands, what it does of its
    /// own accord and an answer to a retry to the users (issues 220, 243,
    /// 284); a command, a request to answer again and an answer to questions
    /// to the agent they name, what a command answers to whom it names, or
    /// to every user (issues 165, 234, 269, 299); an approval through the
    /// approvers - an agent's request to them, theirs to the users, a user's
    /// answer to them, their decision to the users and the agent that asked
    /// (issues 81, 148); an agent's questions to the users, to those asked
    /// alone if they are sensitive (issue 165). Nothing that only asks - a
    /// request to respond, a nudge, a wakeup - nor what the channel or the
    /// moderator say of their own.
    ///
    /// Whether it goes back to its sender is the channel's to decide: only
    /// what it announces in someone's name does.
    pub fn concerns(&self, participant: ActorId) -> bool {
        let sender = self.context.sender_id;
        let user = matches!(participant, ActorId::User(_));
        let agent = matches!(participant, ActorId::Agent(_));
        let approver = matches!(participant, ActorId::Approver(_));
        match &self.event {
            ActorEvent::CompleteMessageGenerated(message) => user || (agent && message.is_heard()),
            ActorEvent::MessageModified(_) | ActorEvent::MessageRevoked { .. } | ActorEvent::MessageReclassified { .. }
            | ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. } => user || agent,
            ActorEvent::TurnFinished { .. } | ActorEvent::TurnGivenUp { .. } | ActorEvent::ResponseStarted { .. } | ActorEvent::StateChanged(_)
            | ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) | ActorEvent::IntermediateMessageGenerated(_)
            | ActorEvent::PlanStepStarted { .. } | ActorEvent::PlanStepFinished { .. } | ActorEvent::ActivityStarted { .. } | ActorEvent::ActivityFinished { .. }
            | ActorEvent::RetryAccepted { .. } | ActorEvent::RetryRejected { .. } | ActorEvent::CommandExecuted { to: None, .. } => user,
            ActorEvent::CommandRequested { agent: named, .. } | ActorEvent::RetryRequested { author: named, .. } => agent && participant == *named,
            ActorEvent::CommandStarted { to, .. } | ActorEvent::CommandExecuted { to: Some(to), .. } | ActorEvent::CommandFailed { to, .. } => user && participant == *to,
            ActorEvent::ApprovalRequested(_) | ActorEvent::ApprovalRequestWithdrawn { .. } => match sender {
                ActorId::Agent(_) => approver,
                ActorId::Approver(_) => user,
                _ => false,
            },
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) | ActorEvent::ApprovalTimedOut(response) => match sender {
                // A timeout is the approver's alone.
                ActorId::User(_) => approver && !matches!(self.event, ActorEvent::ApprovalTimedOut(_)),
                ActorId::Approver(_) => user || participant == ActorId::Agent(response.agent_id),
                _ => false,
            },
            ActorEvent::QuestionRequested(request) => sender.as_agent_id().is_some() && user && request.shown_to().is_none_or(|only| only.contains(&participant)),
            ActorEvent::QuestionClosed { shown_to, .. } => sender.as_agent_id().is_some() && user && shown_to.as_ref().is_none_or(|only| only.contains(&participant)),
            ActorEvent::QuestionAnswered { agent_id, .. } | ActorEvent::QuestionDeclined { agent_id, .. } => sender.as_user_id().is_some() && participant == ActorId::Agent(*agent_id),
            ActorEvent::ResponseRequested | ActorEvent::NudgeRequested { .. } | ActorEvent::WakeupRequested | ActorEvent::ChainCutOff { .. } | ActorEvent::MessageRejected { .. } => false,
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
            ActorEvent::NudgeRequested { participant } => {
                format!("NudgeRequested(sender_id={}, sender_name={}, participant={})", ctx.sender_id, ctx.sender_name, participant)
            }
            ActorEvent::CommandExecuted { command, to, .. } => {
                format!("CommandExecuted(sender_id={}, sender_name={}, command={}, to={})", ctx.sender_id, ctx.sender_name, command, to.map_or("everybody".to_string(), |to| to.to_string()))
            }
            ActorEvent::CommandStarted { command, to, .. } => {
                format!("CommandStarted(sender_id={}, sender_name={}, command={}, to={})", ctx.sender_id, ctx.sender_name, command, to)
            }
            ActorEvent::ActivityStarted { activity, .. } => {
                format!("ActivityStarted(sender_id={}, sender_name={}, activity={})", ctx.sender_id, ctx.sender_name, activity)
            }
            ActorEvent::ActivityFinished { activity, .. } => {
                format!("ActivityFinished(sender_id={}, sender_name={}, activity={})", ctx.sender_id, ctx.sender_name, activity)
            }
            ActorEvent::CommandFailed { command, to, .. } => {
                format!("CommandFailed(sender_id={}, sender_name={}, command={}, to={})", ctx.sender_id, ctx.sender_name, command, to)
            }
            ActorEvent::ParticipantJoined { participant, .. } => {
                format!("ParticipantJoined(sender_id={}, sender_name={}, participant={})", ctx.sender_id, ctx.sender_name, participant)
            }
            ActorEvent::ParticipantLeft { participant, .. } => {
                format!("ParticipantLeft(sender_id={}, sender_name={}, participant={})", ctx.sender_id, ctx.sender_name, participant)
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
            ActorEvent::MessageRejected { rejected, message, .. } => {
                format!("MessageRejected(sender_id={}, rejected={}, mid={:?})", ctx.sender_id, rejected, message)
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
    /// Carrying out a command an awake agent was asked for (issues 269,
    /// 299) - `/compact`, say, which has its model summarise. Between the
    /// turns of its answers, as `Idle` is: a command turn is no answer, and
    /// a request to respond waits until it is done; nor can a cancel end it.
    /// A sleeping agent carries a command out asleep, and stays `Sleeping`.
    CarryingOutCommand,
    /// Doing something of its own accord within its turn, besides answering
    /// (issue 284) - compacting its context: the counterpart of
    /// `CarryingOutCommand`, which it does when asked. What it does is told
    /// by `ActorEvent::ActivityStarted`.
    Busy,
}

impl ActorState {
    /// Every state, in the order of its declaration - its byte
    /// (`#[repr(u8)]`) is its place here.
    const ALL: [ActorState; 9] = [
        ActorState::Idle,
        ActorState::Planning,
        ActorState::ExecutingTools,
        ActorState::Reflecting,
        ActorState::Cancelling,
        ActorState::Sleeping,
        ActorState::WaitingForUser,
        ActorState::CarryingOutCommand,
        ActorState::Busy,
    ];

    /// Whether the state belongs to a turn - an error in it ends the turn,
    /// not the actor - rather than waiting between turns. Carrying out a
    /// command is between turns: it is done while the actor waits.
    pub fn in_turn(self) -> bool {
        !matches!(self, ActorState::Idle | ActorState::Sleeping | ActorState::CarryingOutCommand)
    }

    /// Whether a cancel can still end the turn in this state (issues 235,
    /// 243): not one ending already, nor none.
    pub fn interruptible(self) -> bool {
        matches!(self, ActorState::Planning | ActorState::ExecutingTools | ActorState::WaitingForUser | ActorState::Busy)
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

/// What of an actor is public to everyone in its channel (issues 176, 403):
/// who it is, what it is called and how it is introduced. Whatever is added
/// to it must be public in the same way, since it is given to whom only
/// reads the channel's roster - a moderator's policy, another agent's
/// prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorDescription {
    pub id: ActorId,
    pub name: Name,
    /// An agent as its personality introduces it, a user as they introduce
    /// themselves; `None` for one without an introduction.
    pub introduction: Option<Introduction>,
}

impl ActorDescription {
    /// The handle it is `@`-mentioned by (`mention::handle_of`), or `None`
    /// if it cannot be mentioned: one that is neither a user nor an agent,
    /// and a name that yields none.
    pub fn handle(&self) -> Option<String> {
        matches!(self.id, ActorId::User(_) | ActorId::Agent(_)).then(|| handle_of(self.name.as_str())).filter(|handle| !handle.is_empty())
    }

    /// Whether it is a person: a user, not an agent, an approver or the
    /// system.
    pub fn is_person(&self) -> bool {
        matches!(self.id, ActorId::User(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorState;
    use crate::store::query::meets_exactly_its_equals;
    use crate::message::MessageRole;
    use std::collections::HashSet;

    /// Issue 403: what is public of an actor names its handle only where it
    /// can be mentioned, and says whether it is a person.
    #[test]
    fn a_description_names_a_handle_where_its_actor_can_be_mentioned() {
        let described = |id, name: &str| ActorDescription { id, name: Name::new(name).unwrap(), introduction: None };
        let zoe = described(ActorId::User(UserId::new()), "Zoë Smith");
        assert_eq!((zoe.handle().as_deref(), zoe.is_person()), (Some("zoe-smith"), true));
        let helper = described(ActorId::Agent(AgentId::new()), "Helper");
        assert_eq!((helper.handle().as_deref(), helper.is_person()), (Some("helper"), false));
        let approver = described(ActorId::Approver(ApproverId::new()), "Approver");
        assert_eq!((approver.handle(), approver.is_person()), (None, false), "an approver is not mentioned");
    }

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
        assert!(!ActorState::CarryingOutCommand.in_turn() && !ActorState::CarryingOutCommand.interruptible(), "no answer, and no cancel ends it");
    }

    #[test]
    fn published_events_carry_redacted_messages() {
        use crate::tool::{ToolCall, ToolVisibility};
        let hidden = Message {
            tool_calls: vec![ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let text = Message { content: Some("hi".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };

        assert!(ActorEvent::CompleteMessageGenerated(hidden.clone()).published().is_none(), "nothing left to show");
        assert!(ActorEvent::PartialMessageGenerated(hidden.clone()).published().is_none());
        match (ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![hidden.clone(), text.clone()] }).published() {
            Some(ActorEvent::TurnFinished { messages, .. }) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].id, text.id);
            }
            other => panic!("expected the response, got {other:?}"),
        }
        assert!(
            matches!(ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![hidden] }.published(), Some(ActorEvent::TurnFinished { messages: m, .. }) if m.is_empty()),
            "the end of the turn is still announced"
        );
    }

    /// A channel's participants to deliver to: two agents, two users, an
    /// approver.
    struct Present {
        agents: [ActorId; 2],
        users: [ActorId; 2],
        approver: ActorId,
    }

    impl Present {
        fn new() -> Self {
            let agent = || ActorId::Agent(AgentId::new());
            let user = || ActorId::User(UserId::new());
            Self { agents: [agent(), agent()], users: [user(), user()], approver: ActorId::Approver(ApproverId::new()) }
        }

        /// Whom of them `event` of `sender` concerns (`Envelope::concerns`).
        fn concerned(&self, sender: ActorId, event: ActorEvent) -> HashSet<ActorId> {
            let envelope = Envelope::new(SecurityContext::default(), EventContext { sender_id: sender, ..EventContext::default() }, event);
            self.agents.iter().chain(&self.users).chain([&self.approver]).copied().filter(|id| envelope.concerns(*id)).collect()
        }

        fn users_and_agents(&self) -> HashSet<ActorId> {
            self.users.iter().chain(&self.agents).copied().collect()
        }

        fn users(&self) -> HashSet<ActorId> {
            self.users.iter().copied().collect()
        }
    }

    /// Issues 224, 258, 418: what is said goes to every user, and to every
    /// agent if there is something to hear in it; a change of it, and who
    /// joined or left, to every user and agent - never to the approver.
    #[test]
    fn a_message_concerns_the_users_and_the_agents_that_hear_something_in_it() {
        use crate::tool::{ToolCall, ToolCallResult};
        let present = Present::new();
        let sender = present.agents[0];
        let said = |content: Option<&str>, role: MessageRole| Message { sender_id: sender, role, content: content.map(str::to_string), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let tool = Message { tool_results: vec![ToolCallResult::of(&call, Default::default(), "done".to_string(), Classification::CHANNEL_DEFAULT)], ..said(Some("done"), MessageRole::Tool) };

        assert_eq!(present.concerned(sender, ActorEvent::CompleteMessageGenerated(said(Some("hello"), MessageRole::Assistant))), present.users_and_agents(), "words, to everyone");
        for unheard in [tool, said(None, MessageRole::Assistant), Message { internal: true, ..said(Some("thinking"), MessageRole::Assistant) }] {
            assert_eq!(present.concerned(sender, ActorEvent::CompleteMessageGenerated(unheard)), present.users(), "nothing to hear, the users alone");
        }
        let id = MessageId::new();
        for changed in [
            ActorEvent::MessageModified(said(Some("changed"), MessageRole::Assistant)),
            ActorEvent::MessageRevoked { message: id, at: Utc::now() },
            ActorEvent::MessageReclassified { message: id, classification: Classification::PUBLIC },
            ActorEvent::ParticipantJoined { participant: present.users[1], name: "bob".to_string() },
            ActorEvent::ParticipantLeft { participant: present.agents[1], name: "helper".to_string() },
        ] {
            let kind = changed.kind();
            assert_eq!(present.concerned(sender, changed), present.users_and_agents(), "{kind}");
        }
    }

    /// Issues 220, 243, 258, 284: how an answer comes about, how a turn
    /// ended, where an agent stands and what it does of its own accord are
    /// the users'.
    #[test]
    fn how_an_answer_comes_about_concerns_the_users_alone() {
        let present = Present::new();
        let (turn_id, message) = (TurnId::new(), Message::new(Classification::CHANNEL_DEFAULT));
        let step = |index| (index, 2, "first".to_string());
        let target = RetryTarget::Turn(turn_id);
        for event in [
            ActorEvent::ResponseStarted { turn_id },
            ActorEvent::StateChanged(ActorState::Planning),
            ActorEvent::PartialMessageGenerated(message.clone()),
            ActorEvent::IntermediatePartialGenerated(message.clone()),
            ActorEvent::IntermediateMessageGenerated(message.clone()),
            ActorEvent::PlanStepStarted { turn_id, step_index: step(0).0, total_steps: step(0).1, description: step(0).2 },
            ActorEvent::PlanStepFinished { turn_id, step_index: step(0).0, total_steps: step(0).1, description: step(0).2 },
            ActorEvent::TurnFinished { turn_id, messages: vec![message.clone()] },
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Failed { message: "boom".to_string() } },
            ActorEvent::ActivityStarted { turn_id, activity: "compacting".to_string() },
            ActorEvent::ActivityFinished { turn_id, activity: "compacting".to_string(), output: "done".to_string() },
            ActorEvent::RetryAccepted { target },
            ActorEvent::RetryRejected { target, reason: RetryRefused::NotLast },
        ] {
            let kind = event.kind();
            assert_eq!(present.concerned(present.agents[0], event), present.users(), "{kind} goes to the users, not to the agents");
        }
    }

    /// Issues 234, 269, 299: a command and a request to answer again go to
    /// the agent they name; what a command answers to its issuer, or to
    /// every user - never to an agent.
    #[test]
    fn a_command_concerns_whom_it_names() {
        let present = Present::new();
        let (agent, issuer) = (present.agents[1], present.users[0]);
        let turn_id = Some(TurnId::new());
        let command = |to| ActorEvent::CommandExecuted { command: "wakeup".to_string(), args: String::new(), output: "Woke b.".to_string(), to, turn_id };

        assert_eq!(present.concerned(issuer, ActorEvent::CommandRequested { command: "compact".to_string(), args: String::new(), agent }), HashSet::from([agent]));
        assert_eq!(present.concerned(issuer, ActorEvent::RetryRequested { target: RetryTarget::Message(MessageId::new()), author: agent }), HashSet::from([agent]));
        assert_eq!(present.concerned(issuer, ActorEvent::CommandRequested { command: "compact".to_string(), args: String::new(), agent: issuer }), HashSet::new(), "only an agent carries one out");
        assert_eq!(present.concerned(agent, command(None)), present.users(), "everybody's answer, to the users");
        assert_eq!(present.concerned(agent, command(Some(issuer))), HashSet::from([issuer]), "a private one, to its issuer");
        assert_eq!(present.concerned(agent, ActorEvent::CommandStarted { command: "compact".to_string(), args: String::new(), to: issuer, turn_id: TurnId::new() }), HashSet::from([issuer]));
        assert_eq!(present.concerned(agent, ActorEvent::CommandFailed { command: "compact".to_string(), reason: "no".to_string(), to: issuer, turn_id }), HashSet::from([issuer]));
    }

    /// Issues 81, 148: an approval goes through the approvers - the
    /// agent's request to them, theirs to the users, a user's answer to
    /// them, their decision to the users and the agent that asked.
    #[test]
    fn an_approval_concerns_the_approvers_and_through_them_the_users() {
        use crate::approval::{ApprovalRequest, ApprovalResponse};
        use crate::tool::ToolCall;
        let present = Present::new();
        let (agent, user, approver) = (present.agents[0], present.users[0], present.approver);
        let ActorId::Agent(agent_id) = agent else { unreachable!() };
        let request = || ActorEvent::ApprovalRequested(ApprovalRequest {
            suggested: None, explicit: false, id: Default::default(), session_id: Default::default(), channel_id: Default::default(), agent_id,
            tool_call: ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() },
            description: "test".to_string(),
        });
        let withdrawn = || ActorEvent::ApprovalRequestWithdrawn { id: Default::default() };
        let response = || ApprovalResponse::new(Default::default(), agent_id);

        for asked in [request(), withdrawn()] {
            let kind = asked.kind();
            assert_eq!(present.concerned(agent, asked), HashSet::from([approver]), "{kind}: internal, not shown to the users yet");
        }
        for asked in [request(), withdrawn()] {
            assert_eq!(present.concerned(approver, asked), present.users(), "what the users act on, or no longer");
        }
        assert!(present.concerned(user, withdrawn()).is_empty(), "only an agent and the approver withdraw");
        assert_eq!(present.concerned(user, ActorEvent::ApprovalGranted(response())), HashSet::from([approver]), "a user's answer, to the approver alone");
        assert!(present.concerned(user, ActorEvent::ApprovalTimedOut(response())).is_empty(), "a timeout is the approver's alone");
        let decided = present.users().into_iter().chain([agent]).collect::<HashSet<_>>();
        for decision in [ActorEvent::ApprovalGranted(response()), ActorEvent::ApprovalDenied(response()), ActorEvent::ApprovalTimedOut(response())] {
            let kind = decision.kind();
            assert_eq!(present.concerned(approver, decision), decided, "{kind} ends the approval for the users, and goes to the agent that asked");
        }
    }

    /// Issue 165: an agent's questions, and their closing, go to the users,
    /// to those asked alone if sensitive; a user's answer to the agent that
    /// asked alone.
    #[test]
    fn questions_concern_the_users_asked_and_the_answers_the_agent_asking() {
        use crate::question::{Addressee, Question, QuestionId, QuestionRequest, Questions};
        use crate::tool::ToolCall;
        let present = Present::new();
        let (agent, alice) = (present.agents[0], present.users[0]);
        let ActorId::Agent(agent_id) = agent else { unreachable!() };
        let request = |sensitive| ActorEvent::QuestionRequested(QuestionRequest {
            id: QuestionId::new(), session_id: Default::default(), channel_id: Default::default(), agent_id,
            tool_call: ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() },
            questions: Questions {
                questions: vec![Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }],
                ask: vec![Addressee::Actor(alice)],
                sensitive,
            },
        });
        let closed = |shown_to| ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to };

        assert_eq!(present.concerned(agent, request(false)), present.users(), "every user sees what the agent waits for");
        assert_eq!(present.concerned(agent, request(true)), HashSet::from([alice]), "sensitive: those asked alone");
        assert!(present.concerned(alice, request(false)).is_empty(), "only an agent asks");
        assert_eq!(present.concerned(agent, closed(None)), present.users());
        assert_eq!(present.concerned(agent, closed(Some(vec![alice]))), HashSet::from([alice]));
        let answered = ActorEvent::QuestionAnswered { id: QuestionId::new(), agent_id, responses: Vec::new() };
        assert_eq!(present.concerned(alice, answered.clone()), HashSet::from([agent]), "neither the other agent, the other users nor the approver");
        assert_eq!(present.concerned(alice, ActorEvent::QuestionDeclined { id: QuestionId::new(), agent_id }), HashSet::from([agent]));
        assert!(present.concerned(present.agents[1], answered).is_empty(), "only a user answers");
    }

    /// Issues 200, 278, 304, 418: what only asks, and what the channel or
    /// the moderator say of their own, concerns nobody by the rule - the
    /// moderator and the channel name whom.
    #[test]
    fn what_only_asks_concerns_nobody() {
        let present = Present::new();
        let user = present.users[0];
        for event in [
            ActorEvent::ResponseRequested,
            ActorEvent::WakeupRequested,
            ActorEvent::NudgeRequested { participant: present.agents[0] },
            ActorEvent::ChainCutOff { message: MessageId::new(), max_agent_hops: 3, not_asked: Vec::new() },
            ActorEvent::MessageRejected { rejected: "CompleteMessageGenerated".to_string(), message: None, reason: Rejection::Forbidden },
        ] {
            let kind = event.kind();
            assert!(present.concerned(user, event).is_empty(), "{kind}");
        }
    }

    /// An actor as a query compares it (issue 293): the variant counts, not
    /// only the UUID.
    #[test]
    fn an_actor_meets_exactly_the_actors_equal_to_it() {
        let id = Uuid::new_v4();
        meets_exactly_its_equals(&[
            ActorId::None, ActorId::System,
            ActorId::User(UserId(id)), ActorId::User(UserId::new()),
            ActorId::Agent(AgentId(id)), ActorId::Approver(ApproverId(id)),
        ]);
    }

}
