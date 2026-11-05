//! What happens in a channel as an actor sends it or is sent it (`ActorEvent`): messages and turns,
//! commands, approvals and questions - with why an actor is asked to answer (`RequestReason`), what
//! an answer asked for again is about (`RetryTarget`), and why the channel did not let an event in
//! (`Rejection`).

use super::{ActorId, ActorState};
use crate::agent::{AgentId, RetryRefused};
use crate::approval::{ApprovalId, ApprovalRequest, ApprovalResponse};
use crate::chain::Position;
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::question::{QuestionId, QuestionRequest, Response};
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

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
    ApprovalRequestWithdrawn {
        id: ApprovalId,
    },
    /// An agent's call asks its questions (issue 165): sent by the agent to
    /// the users - to those asked alone if the questions are sensitive.
    QuestionRequested(QuestionRequest),
    /// A user's answers to the questions `id` of agent `agent_id`, sent to
    /// that agent alone; who gave them is the envelope's sender.
    QuestionAnswered {
        id: QuestionId,
        agent_id: AgentId,
        responses: Vec<Response>,
    },
    /// A user declines to answer the questions `id` of agent `agent_id` -
    /// by saying so, or by writing a message instead - sent to that agent
    /// alone.
    QuestionDeclined {
        id: QuestionId,
        agent_id: AgentId,
    },
    /// The agent no longer waits for the answers to its questions `id`:
    /// they were answered or declined, or its turn ended first. Sent to the
    /// users it asked them of - `shown_to`, or every user - so that their
    /// frontends no longer offer to answer.
    QuestionClosed {
        id: QuestionId,
        shown_to: Option<Vec<ActorId>>,
    },
    /// Asks its receiver to respond to the event whose context it carries
    /// (issue 45), for `reason` (issue 439): built by the channel for each
    /// its moderator asks, and recorded in the system's name to them
    /// (`HistoryEntry::ResponseRequested`, issue 417).
    ResponseRequested {
        reason: RequestReason,
    },
    /// An answer begins: the turn `turn_id` of the sender (issue 235) - an
    /// agent's turn or an external agent's work - whose messages carry the
    /// same id. Announced before anything of it, so that an answer that is
    /// not streamed is known as under way, and which turn it is, before it
    /// ends.
    ResponseStarted {
        turn_id: TurnId,
    },
    /// The sender now stands at this state (issue 243): announced by an
    /// agent as it changes, so that the users see who works, sleeps or
    /// waits for an approval. Sent in the agent's own context, not a
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
    TurnFinished {
        turn_id: TurnId,
        messages: Vec<Message>,
    },
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
    NudgeRequested {
        participant: ActorId,
    },
    /// A command was carried out (issues 28, 269): `command` is its name
    /// without the slash, `args` what it was given, `output` what it
    /// answered. One of the session - `/wakeup` - the envelope's sender is
    /// the user who ran it, sent only when the answer concerns everybody;
    /// one an agent carried out, the sender is the agent, `turn_id` the
    /// turn it was carried out in, and `to` the issuer alone if the answer
    /// is theirs, `None` if it is everybody's - the issuer is the event
    /// context's initiator either way.
    ///
    /// Shown to users, never heard by agents. Recorded by the channel with
    /// its output, to whom it went (`HistoryEntry::CommandExecuted`, issue
    /// 417).
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
    /// issuer so (`CommandStarted`). Shown to the issuer alone, and
    /// recorded to them alone (`HistoryEntry::CommandFailed`, issue 417):
    /// nothing else happened.
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
    /// `participant`, called `name`, joined the channel (issue 408). The envelope's sender is who
    /// added it - the participant itself if it joined by itself, the kernel, `ActorId::System`, if
    /// an agent was restored. Sent by the channel alone, as the participant's actor starts, so that
    /// it is the first thing the newcomer hears.
    ///
    /// The channel records it in its history (`HistoryEntry::
    /// ParticipantJoined`), and everybody is told - the agents too, which
    /// take note of it without responding -, also whose name it is told in.
    ParticipantJoined {
        participant: ActorId,
        name: String,
    },
    /// `participant`, called `name`, left the channel (issues 162, 408). The envelope's sender is
    /// who removed it - the kernel itself, `ActorId::System`, if nobody did. Sent by the channel
    /// alone, once the participant's actor no longer runs in it: nothing of the participant
    /// follows, so whatever of it was under way - a turn, a call of a tool - has ended.
    ///
    /// Recorded and told as `ParticipantJoined` is. The channel delivered
    /// nothing to it from the position `until` on (issue 417): where it
    /// stopped, which its history records, so that it says to the position
    /// whom the channel delivered what.
    ParticipantLeft {
        participant: ActorId,
        name: String,
        until: Position,
    },
    /// `participant` is given the channel's history from the position `from`
    /// on as it joins (issues 46, 417): how much of it `by`, called
    /// `by_name`, who added it, granted. Sent by the channel alone, in the
    /// system's name, right before `ParticipantJoined`, to the newcomer
    /// alone, and only if it shares something; recorded in its history
    /// (`HistoryEntry::HistoryShared`), so that what the newcomer was
    /// delivered as it joined is read from there. Its actor has taken that
    /// part in already as it was started (`Actor::catch_up`).
    HistoryShared {
        participant: ActorId,
        from: Position,
        by: ActorId,
        by_name: String,
    },
    /// A chain between agents reached its limit (issue 200): the response
    /// ending in `message` was delivered, but it made nobody respond, since
    /// that would have been more than `max_agent_hops` hops since the last
    /// user's message. `not_asked` are the agents it mentioned, empty if it
    /// mentioned none. Sent by the moderator in the name of
    /// `ActorId::System`, to the users of the channel and to the agents
    /// involved: the sender of the response and those it mentioned.
    ///
    /// Recorded by the channel to whom it was told
    /// (`HistoryEntry::ChainCutOff`, issue 417); an agent takes no note of
    /// it.
    ChainCutOff {
        message: MessageId,
        max_agent_hops: u32,
        not_asked: Vec<ActorId>,
    },
    /// The responder policy could not choose who responds to the response ending in `message`
    /// (issue 441), and nobody was asked: the response was delivered, but nobody answers it unless
    /// someone is mentioned. Sent by the moderator in the name of `ActorId::System`, to the users
    /// of the channel.
    ///
    /// Recorded by the channel to whom it was told (`HistoryEntry::ResponderUndecided`); an agent
    /// takes no note of it.
    ResponderUndecided {
        message: MessageId,
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
    /// own. Shown to users, and recorded (`HistoryEntry::RetryAccepted`,
    /// issue 417).
    RetryAccepted {
        target: RetryTarget,
    },
    /// The agent does not answer again instead of `target`, for `reason`
    /// (issue 234) - one of a known few, which a user interface words
    /// itself. Shown to users, and recorded (`HistoryEntry::RetryRejected`,
    /// issue 417).
    RetryRejected {
        target: RetryTarget,
        reason: RetryRefused,
    },
    /// The agent was given `message` to read (issues 22, 417) - a skill
    /// loaded for it, what a compaction gave again -, as its transcript
    /// records it: sent by the agent alone, so that its channel's history
    /// holds what it read where it read it. The message is the agent's
    /// record, not one it said - its sender is whoever gave it -, so it is
    /// no message the event carries (`messages`). Delivered to nobody.
    MessageGiven {
        message: Message,
    },
    /// The agent no longer has `message` to read, given to it before
    /// (`MessageGiven`), from `at` on (issue 537) - a prompt part that no
    /// longer holds -, as its transcript records it: sent by the agent
    /// alone, so that its channel's history holds where it stopped reading
    /// it. Delivered to nobody.
    MessageWithdrawn {
        message: MessageId,
        at: DateTime<Utc>,
    },
    /// The agent replaced its context `from` by `to` in its turn `turn_id`
    /// (issues 16, 417): what its transcript records, told to its channel's
    /// history; what replaced the older messages follows as what it was
    /// given (`MessageGiven`). Delivered to nobody - the users are told what
    /// came of it (`ActivityFinished`).
    ContextCompacted {
        turn_id: TurnId,
        from: Digest,
        to: Digest,
    },
    /// The channel did not let in an event of the receiver's own (issue
    /// 304): `rejected` names its kind (`ActorEvent::kind`), `message` the
    /// message it was or was about, if any, and `reason` is one of a known
    /// few, which a user interface words itself - the channel's one decision
    /// of what enters it, made in order as it takes the event in. Sent by the
    /// channel to the event's sender alone, and recorded to it alone
    /// (`HistoryEntry::MessageRejected`, issue 417); what was rejected
    /// entered nothing.
    MessageRejected {
        rejected: String,
        message: Option<MessageId>,
        reason: Rejection,
    },
}

/// Why a moderator asks an actor to respond (issue 439). Ordered by how much
/// the answer is wanted: an agent that takes several requests into one turn
/// answers for the strongest reason among them. The default is the weakest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestReason {
    /// Its `ResponderPolicy` chose it: nobody mentioned it, and whether it
    /// has anything to add is the policy's guess.
    #[default]
    Chosen,
    /// Its `ResponderPolicy` could not choose, and the moderator asks every agent the policy could
    /// have chosen, so that what was said is not left unanswered (issue 441).
    Undecided,
    /// A user nudged it to go on (issue 278).
    Nudged,
    /// What it responds to mentions it, by its handle or with `@all`.
    Mentioned,
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
        let publish =
            |message: Message, wrap: fn(Message) -> ActorEvent| message.published().map(wrap);
        match self {
            ActorEvent::PartialMessageGenerated(m) => {
                publish(m, ActorEvent::PartialMessageGenerated)
            }
            ActorEvent::CompleteMessageGenerated(m) => {
                publish(m, ActorEvent::CompleteMessageGenerated)
            }
            ActorEvent::IntermediatePartialGenerated(m) => {
                publish(m, ActorEvent::IntermediatePartialGenerated)
            }
            ActorEvent::IntermediateMessageGenerated(m) => {
                publish(m, ActorEvent::IntermediateMessageGenerated)
            }
            ActorEvent::MessageModified(m) => publish(m, ActorEvent::MessageModified),
            ActorEvent::TurnFinished { turn_id, messages } => Some(ActorEvent::TurnFinished {
                turn_id,
                messages: messages.iter().filter_map(Message::published).collect(),
            }),
            // The agent's record of what it read and did to its context.
            ActorEvent::MessageGiven { .. }
            | ActorEvent::MessageWithdrawn { .. }
            | ActorEvent::ContextCompacted { .. } => None,
            other => Some(other),
        }
    }

    /// Whether it may be lost on its way to an actor whose inbox has no
    /// room (issue 223): what streams, where an actor stands, the start of
    /// what ends with an event the history records, and what only asks to
    /// be woken or asks for a response, which the channel records as the
    /// request it becomes. What the history records is delivered from there
    /// instead; the answers to questions, which it never records (issue
    /// 165), are not lost.
    pub fn is_transient(&self) -> bool {
        match self {
            ActorEvent::PartialMessageGenerated(_)
            | ActorEvent::IntermediatePartialGenerated(_)
            | ActorEvent::StateChanged(_)
            | ActorEvent::ResponseStarted { .. }
            | ActorEvent::CommandStarted { .. }
            | ActorEvent::ActivityStarted { .. }
            | ActorEvent::PlanStepFinished { .. }
            | ActorEvent::WakeupRequested
            | ActorEvent::NudgeRequested { .. } => true,
            ActorEvent::ApprovalRequested(_)
            | ActorEvent::ApprovalGranted(_)
            | ActorEvent::ApprovalDenied(_)
            | ActorEvent::ApprovalTimedOut(_)
            | ActorEvent::ApprovalRequestWithdrawn { .. }
            | ActorEvent::QuestionRequested(_)
            | ActorEvent::QuestionAnswered { .. }
            | ActorEvent::QuestionDeclined { .. }
            | ActorEvent::QuestionClosed { .. }
            | ActorEvent::ResponseRequested { .. }
            | ActorEvent::CompleteMessageGenerated(_)
            | ActorEvent::TurnFinished { .. }
            | ActorEvent::IntermediateMessageGenerated(_)
            | ActorEvent::PlanStepStarted { .. }
            | ActorEvent::TurnGivenUp { .. }
            | ActorEvent::CommandRequested { .. }
            | ActorEvent::CommandExecuted { .. }
            | ActorEvent::CommandFailed { .. }
            | ActorEvent::ActivityFinished { .. }
            | ActorEvent::ParticipantJoined { .. }
            | ActorEvent::ParticipantLeft { .. }
            | ActorEvent::HistoryShared { .. }
            | ActorEvent::MessageModified(_)
            | ActorEvent::MessageRevoked { .. }
            | ActorEvent::MessageReclassified { .. }
            | ActorEvent::RetryRequested { .. }
            | ActorEvent::RetryAccepted { .. }
            | ActorEvent::RetryRejected { .. }
            | ActorEvent::ChainCutOff { .. }
            | ActorEvent::ResponderUndecided { .. }
            | ActorEvent::MessageRejected { .. }
            | ActorEvent::MessageGiven { .. }
            | ActorEvent::MessageWithdrawn { .. }
            | ActorEvent::ContextCompacted { .. } => false,
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
            ActorEvent::MessageRevoked { message, .. }
            | ActorEvent::MessageReclassified { message, .. } => Some(*message),
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), .. } => {
                Some(*message)
            }
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
    /// joined or left (issue 408), what a newcomer was given of the history
    /// (issue 417). It reaches whose name it is told in,
    /// too - nothing of theirs is echoed back to them.
    pub fn is_announcement(&self) -> bool {
        matches!(
            self,
            ActorEvent::ParticipantJoined { .. }
                | ActorEvent::ParticipantLeft { .. }
                | ActorEvent::HistoryShared { .. }
        )
    }

    /// Why a request to respond asks (issue 439); `None` for every other
    /// event.
    pub fn request_reason(&self) -> Option<RequestReason> {
        match self {
            ActorEvent::ResponseRequested { reason } => Some(*reason),
            _ => None,
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
            ActorEvent::ResponseRequested { .. } => "ResponseRequested",
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
            ActorEvent::HistoryShared { .. } => "HistoryShared",
            ActorEvent::ChainCutOff { .. } => "ChainCutOff",
            ActorEvent::ResponderUndecided { .. } => "ResponderUndecided",
            ActorEvent::MessageModified(_) => "MessageModified",
            ActorEvent::MessageRevoked { .. } => "MessageRevoked",
            ActorEvent::MessageReclassified { .. } => "MessageReclassified",
            ActorEvent::RetryRequested { .. } => "RetryRequested",
            ActorEvent::RetryAccepted { .. } => "RetryAccepted",
            ActorEvent::RetryRejected { .. } => "RetryRejected",
            ActorEvent::MessageRejected { .. } => "MessageRejected",
            ActorEvent::MessageGiven { .. } => "MessageGiven",
            ActorEvent::MessageWithdrawn { .. } => "MessageWithdrawn",
            ActorEvent::ContextCompacted { .. } => "ContextCompacted",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_events_carry_redacted_messages() {
        use crate::tool::{ToolCall, ToolVisibility};
        let hidden = Message {
            tool_calls: vec![ToolCall {
                id: None,
                function_name: "f".to_string(),
                arguments: serde_json::json!({}),
                visibility: ToolVisibility::HIDDEN,
                presentation: Default::default(),
            }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let text = Message {
            content: Some("hi".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };

        assert!(
            ActorEvent::CompleteMessageGenerated(hidden.clone()).published().is_none(),
            "nothing left to show"
        );
        assert!(ActorEvent::PartialMessageGenerated(hidden.clone()).published().is_none());
        match (ActorEvent::TurnFinished {
            turn_id: TurnId::new(),
            messages: vec![hidden.clone(), text.clone()],
        })
        .published()
        {
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
}
