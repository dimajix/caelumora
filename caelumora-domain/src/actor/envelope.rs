//! An event on its way through a channel (`Envelope`): when it was sent, where and in which chain
//! (`EventContext`), whom it goes to and what is kept of it - and what an actor does with one it
//! looks at in its inbox (`Pick`).

use super::{ActorEvent, ActorId};
use crate::approval::ApprovalStatus;
use crate::chain::Position;
use crate::channel::ChannelId;
use crate::history::{HistoryEntry, Recipients};
use crate::session::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// The position in its channel's history of the entry it was delivered
    /// as (issue 417), stamped by the channel as it delivers it: what a
    /// client tells the live event from the item it reads later by. `None`
    /// for what the history does not record, and for what was not
    /// delivered by a channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// How many times since the last message of a user an agent's message
    /// made another agent respond (issue 45). A user's message starts at 0;
    /// `next` keeps it, and the moderator counts a hop when it asks an agent
    /// to respond to an agent. Absent in what was stored before it existed.
    #[serde(default)]
    pub agent_hops: u32,
    /// Who caused it - the root of its chain: whoever asked, or whoever
    /// asked whoever asked, and its sender itself when nobody did (issue
    /// 432). `next` keeps it. What the chain is read by - whose authority a
    /// turn hears in (issue 431), who asked for a command carried out -;
    /// the `SecurityContext` names it as well, for the checks made in it.
    pub initiator_id: ActorId,
}

impl EventContext {
    pub fn new(
        session_id: SessionId,
        channel_id: ChannelId,
        sender_id: ActorId,
        sender_name: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            channel_id,
            sender_id,
            sender_name: sender_name.into(),
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::default(),
            span_id: Uuid::new_v4(),
            span_depth: 0,
            position: None,
            agent_hops: 0,
            initiator_id: sender_id,
        }
    }

    pub fn next(&self, sender_id: ActorId, sender_name: impl Into<String>) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            sender_id,
            sender_name: sender_name.into(),
            event_id: Uuid::new_v4(),
            correlation_id: self.event_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
            position: None,
            agent_hops: self.agent_hops,
            initiator_id: self.initiator_id,
        }
    }
}

/// What is done with an envelope an actor looks at in its inbox, looking for the next it takes
/// without waiting (`EventReceiver::try_recv_if` of the kernel, issue 435).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// It is taken - ahead of what was passed over before it, if anything was.
    Take,
    /// It stays where it is, to be taken later, and what follows it is looked at.
    PassOver,
    /// It stays where it is, and so does everything behind it: nothing past it is taken.
    Stop,
}

/// An event in a channel, with when it was sent and where it happens and in which chain
/// (`EventContext`). It carries no security context: in whose name it is checked the channel
/// builds itself, as it admits it, from the sender it names (issue 516).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub timestamp: DateTime<Utc>,
    pub context: EventContext,
    pub event: ActorEvent,
}

impl Envelope {
    pub fn new(context: EventContext, event: ActorEvent) -> Self {
        Self { timestamp: Utc::now(), context, event }
    }

    /// The same envelope - sender, context - with `event` instead, e.g. a
    /// moderator passing an answer on.
    pub fn forward(&self, event: ActorEvent) -> Self {
        Self { timestamp: Utc::now(), context: self.context.clone(), event }
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
    /// where a step of its plan began (issue 263); a command carried out,
    /// with its answer, or not (issue 269) - who asked is the initiator;
    /// what came of what an agent did of its own accord (issue 284); who
    /// joined or left, by its sender (issue 408); a change of a message
    /// (issue 234); what an agent was given and a compaction of its context;
    /// the answer to a request to answer again, a chain cut off, a rejection
    /// and a request to respond (issue 417); what an actor's state depends
    /// on receiving, to be delivered again from the history (issue 223) - a
    /// command and a request to answer again asked of an agent, approvals,
    /// questions asked and closed. Every mark in the name of its sender,
    /// when it was sent. `None` for what the history does not record: what
    /// streams, a beginning whose end is recorded, where an actor stands, a
    /// nudge - recorded as the request it becomes -, and the answers to
    /// questions, which go to the tool alone (issue 165).
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    pub fn entry(&self) -> Option<HistoryEntry> {
        let actor = self.context.sender_id;
        let at = self.timestamp;
        Some(match &self.event {
            ActorEvent::CompleteMessageGenerated(message)
            | ActorEvent::IntermediateMessageGenerated(message) => {
                HistoryEntry::Said { message: message.clone() }
            }
            ActorEvent::MessageGiven { message } => {
                HistoryEntry::Given { message: message.clone() }
            }
            ActorEvent::ContextCompacted { turn_id, from, to } => {
                HistoryEntry::Compacted { turn_id: *turn_id, actor, from: *from, to: *to, at }
            }
            ActorEvent::TurnFinished { turn_id, .. } => {
                HistoryEntry::TurnFinished { turn_id: *turn_id, actor, at }
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                HistoryEntry::TurnGivenUp { turn_id: *turn_id, actor, reason: reason.clone(), at }
            }
            ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description } => {
                HistoryEntry::PlanStepStarted {
                    turn_id: *turn_id,
                    actor,
                    step_index: *step_index,
                    total_steps: *total_steps,
                    description: description.clone(),
                    at,
                }
            }
            ActorEvent::CommandExecuted { command, args, output, to, turn_id } => {
                HistoryEntry::CommandExecuted {
                    turn_id: *turn_id,
                    actor,
                    issuer: self.context.initiator_id,
                    command: command.clone(),
                    args: args.clone(),
                    output: output.clone(),
                    to: *to,
                    at,
                }
            }
            ActorEvent::CommandFailed { command, reason, to, turn_id } => {
                HistoryEntry::CommandFailed {
                    turn_id: *turn_id,
                    actor,
                    command: command.clone(),
                    reason: reason.clone(),
                    to: *to,
                    at,
                }
            }
            ActorEvent::RetryAccepted { target } => {
                HistoryEntry::RetryAccepted { target: *target, actor, at }
            }
            ActorEvent::RetryRejected { target, reason } => {
                HistoryEntry::RetryRejected { target: *target, reason: *reason, actor, at }
            }
            ActorEvent::ChainCutOff { message, max_agent_hops, not_asked } => {
                HistoryEntry::ChainCutOff {
                    message: *message,
                    max_agent_hops: *max_agent_hops,
                    not_asked: not_asked.clone(),
                    at,
                }
            }
            ActorEvent::ResponderUndecided { message } => {
                HistoryEntry::ResponderUndecided { message: *message, at }
            }
            ActorEvent::MessageRejected { rejected, message, reason } => {
                HistoryEntry::MessageRejected {
                    rejected: rejected.clone(),
                    message: *message,
                    reason: *reason,
                    at,
                }
            }
            ActorEvent::ResponseRequested { reason } => {
                HistoryEntry::ResponseRequested { by: actor, reason: *reason, at }
            }
            ActorEvent::ActivityFinished { turn_id, activity, output } => {
                HistoryEntry::ActivityFinished {
                    turn_id: *turn_id,
                    actor,
                    activity: activity.clone(),
                    output: output.clone(),
                    at,
                }
            }
            ActorEvent::ParticipantJoined { participant, name } => {
                HistoryEntry::ParticipantJoined {
                    participant: *participant,
                    name: name.clone(),
                    by: actor,
                    by_name: self.context.sender_name.clone(),
                    at,
                }
            }
            ActorEvent::ParticipantLeft { participant, name, until } => {
                HistoryEntry::ParticipantLeft {
                    participant: *participant,
                    name: name.clone(),
                    by: actor,
                    by_name: self.context.sender_name.clone(),
                    until: *until,
                    at,
                }
            }
            ActorEvent::HistoryShared { participant, from, by, by_name } => {
                HistoryEntry::HistoryShared {
                    participant: *participant,
                    from: *from,
                    by: *by,
                    by_name: by_name.clone(),
                    at,
                }
            }
            ActorEvent::MessageModified(modified) => {
                HistoryEntry::Modified { message: modified.clone() }
            }
            ActorEvent::MessageRevoked { message, at } => {
                HistoryEntry::Revoked { message: *message, at: *at }
            }
            ActorEvent::MessageReclassified { message, classification } => {
                HistoryEntry::Reclassified {
                    message: *message,
                    classification: classification.clone(),
                }
            }
            ActorEvent::CommandRequested { command, args, agent } => {
                HistoryEntry::CommandRequested {
                    agent: *agent,
                    command: command.clone(),
                    args: args.clone(),
                    by: actor,
                    at,
                }
            }
            ActorEvent::RetryRequested { target, author } => {
                HistoryEntry::RetryRequested { target: *target, author: *author, by: actor, at }
            }
            ActorEvent::ApprovalRequested(request) => {
                HistoryEntry::ApprovalRequested { request: request.clone(), by: actor, at }
            }
            ActorEvent::ApprovalGranted(response) => HistoryEntry::ApprovalDecided {
                response: response.clone(),
                status: ApprovalStatus::Approved,
                by: actor,
                at,
            },
            ActorEvent::ApprovalDenied(response) => HistoryEntry::ApprovalDecided {
                response: response.clone(),
                status: ApprovalStatus::Denied,
                by: actor,
                at,
            },
            ActorEvent::ApprovalTimedOut(response) => HistoryEntry::ApprovalDecided {
                response: response.clone(),
                status: ApprovalStatus::Timeout,
                by: actor,
                at,
            },
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                HistoryEntry::ApprovalRequestWithdrawn { id: *id, by: actor, at }
            }
            ActorEvent::QuestionRequested(request) => {
                HistoryEntry::QuestionRequested { request: request.clone(), by: actor, at }
            }
            ActorEvent::QuestionClosed { id, shown_to } => {
                HistoryEntry::QuestionClosed { id: *id, shown_to: shown_to.clone(), by: actor, at }
            }
            // The answers to questions go to the tool alone, never into a
            // history (issue 165).
            ActorEvent::QuestionAnswered { .. }
            | ActorEvent::QuestionDeclined { .. }
            | ActorEvent::ResponseStarted { .. }
            | ActorEvent::StateChanged(_)
            | ActorEvent::PartialMessageGenerated(_)
            | ActorEvent::IntermediatePartialGenerated(_)
            | ActorEvent::WakeupRequested
            | ActorEvent::PlanStepFinished { .. }
            | ActorEvent::NudgeRequested { .. }
            | ActorEvent::CommandStarted { .. }
            | ActorEvent::ActivityStarted { .. } => return None,
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

    /// Whom the envelope goes to by the rule every channel delivers by
    /// (issue 418), unless a moderator decides otherwise. What the history
    /// records goes to whom its entry goes to (`HistoryEntry::recipients`,
    /// issue 223): one rule for what is delivered and what is read later.
    /// Of the rest, how an answer comes about, where an agent stands and the
    /// start of what it does of its own accord go to the users (issues 220,
    /// 243, 284); the start of a command to whom asked for it (issue 299);
    /// a user's answers to questions to the agent that asked them (issue
    /// 165). Nothing that only asks - a nudge, a wakeup.
    ///
    /// Whether it goes back to its sender is the channel's to decide: only
    /// what it announces in someone's name does.
    pub fn recipients(&self) -> Recipients {
        if let Some(entry) = self.entry() {
            return entry.recipients();
        }
        let sender = self.context.sender_id;
        match &self.event {
            ActorEvent::ResponseStarted { .. }
            | ActorEvent::StateChanged(_)
            | ActorEvent::PartialMessageGenerated(_)
            | ActorEvent::IntermediatePartialGenerated(_)
            | ActorEvent::PlanStepFinished { .. }
            | ActorEvent::ActivityStarted { .. } => Recipients::members(false),
            ActorEvent::CommandStarted { to, .. } => Recipients::users(Some(vec![*to])),
            ActorEvent::QuestionAnswered { agent_id, .. }
            | ActorEvent::QuestionDeclined { agent_id, .. }
                if sender.as_user_id().is_some() =>
            {
                Recipients::only(vec![ActorId::Agent(*agent_id)])
            }
            // What only asks, and what is sent in a name it may not be.
            _ => Recipients::NOBODY,
        }
    }

    /// Short, human-readable summary of this envelope's event, used for logging.
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    pub fn describe(&self) -> String {
        let ctx = &self.context;
        match &self.event {
            ActorEvent::PartialMessageGenerated(m) => {
                format!(
                    "Partial(sender_id={}, sender_name={}, role={:?}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.role, m.id
                )
            }
            ActorEvent::CompleteMessageGenerated(m) => {
                format!(
                    "Complete(sender_id={}, sender_name={}, role={:?}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.role, m.id
                )
            }
            ActorEvent::TurnFinished { messages: v, .. } => match v.last() {
                Some(m) => format!(
                    "EndOfTurn(sender_id={}, sender_name={}, role={:?}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.role, m.id
                ),
                None => format!(
                    "EndOfTurn(sender_id={}, sender_name={}, no message)",
                    ctx.sender_id, ctx.sender_name
                ),
            },
            ActorEvent::ResponseRequested { reason } => {
                format!(
                    "ResponseRequested(sender_id={}, sender_name={}, reason={:?})",
                    ctx.sender_id, ctx.sender_name, reason
                )
            }
            ActorEvent::IntermediatePartialGenerated(m) => {
                format!(
                    "IntermediatePartial(sender_id={}, sender_name={}, role={:?}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.role, m.id
                )
            }
            ActorEvent::IntermediateMessageGenerated(m) => {
                format!(
                    "IntermediateMessage(sender_id={}, sender_name={}, role={:?}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.role, m.id
                )
            }
            ActorEvent::PlanStepStarted { step_index, total_steps, .. } => {
                format!(
                    "PlanStepStarted(sender_id={}, sender_name={}, step={}/{})",
                    ctx.sender_id,
                    ctx.sender_name,
                    step_index + 1,
                    total_steps
                )
            }
            ActorEvent::PlanStepFinished { step_index, total_steps, .. } => {
                format!(
                    "PlanStepFinished(sender_id={}, sender_name={}, step={}/{})",
                    ctx.sender_id,
                    ctx.sender_name,
                    step_index + 1,
                    total_steps
                )
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                format!(
                    "TurnGivenUp(sender_id={}, sender_name={}, turn_id={}, reason={})",
                    ctx.sender_id,
                    ctx.sender_name,
                    turn_id,
                    reason.cause()
                )
            }
            ActorEvent::CommandRequested { command, agent, .. } => {
                format!(
                    "CommandRequested(sender_id={}, sender_name={}, command={}, agent={})",
                    ctx.sender_id, ctx.sender_name, command, agent
                )
            }
            ActorEvent::NudgeRequested { participant } => {
                format!(
                    "NudgeRequested(sender_id={}, sender_name={}, participant={})",
                    ctx.sender_id, ctx.sender_name, participant
                )
            }
            ActorEvent::CommandExecuted { command, to, .. } => {
                format!(
                    "CommandExecuted(sender_id={}, sender_name={}, command={}, to={})",
                    ctx.sender_id,
                    ctx.sender_name,
                    command,
                    to.map_or("everybody".to_string(), |to| to.to_string())
                )
            }
            ActorEvent::CommandStarted { command, to, .. } => {
                format!(
                    "CommandStarted(sender_id={}, sender_name={}, command={}, to={})",
                    ctx.sender_id, ctx.sender_name, command, to
                )
            }
            ActorEvent::ActivityStarted { activity, .. } => {
                format!(
                    "ActivityStarted(sender_id={}, sender_name={}, activity={})",
                    ctx.sender_id, ctx.sender_name, activity
                )
            }
            ActorEvent::ActivityFinished { activity, .. } => {
                format!(
                    "ActivityFinished(sender_id={}, sender_name={}, activity={})",
                    ctx.sender_id, ctx.sender_name, activity
                )
            }
            ActorEvent::CommandFailed { command, to, .. } => {
                format!(
                    "CommandFailed(sender_id={}, sender_name={}, command={}, to={})",
                    ctx.sender_id, ctx.sender_name, command, to
                )
            }
            ActorEvent::ParticipantJoined { participant, .. } => {
                format!(
                    "ParticipantJoined(sender_id={}, sender_name={}, participant={})",
                    ctx.sender_id, ctx.sender_name, participant
                )
            }
            ActorEvent::ParticipantLeft { participant, .. } => {
                format!(
                    "ParticipantLeft(sender_id={}, sender_name={}, participant={})",
                    ctx.sender_id, ctx.sender_name, participant
                )
            }
            ActorEvent::HistoryShared { participant, from, by, .. } => {
                format!(
                    "HistoryShared(sender_id={}, participant={}, from={}, by={})",
                    ctx.sender_id, participant, from, by
                )
            }
            ActorEvent::ChainCutOff { message, max_agent_hops, not_asked } => {
                format!(
                    "ChainCutOff(sender_id={}, message={}, max_agent_hops={}, not_asked={})",
                    ctx.sender_id,
                    message,
                    max_agent_hops,
                    not_asked.len()
                )
            }
            ActorEvent::ResponderUndecided { message } => {
                format!("ResponderUndecided(sender_id={}, message={})", ctx.sender_id, message)
            }
            ActorEvent::MessageModified(m) => {
                format!(
                    "MessageModified(sender_id={}, sender_name={}, mid={})",
                    ctx.sender_id, ctx.sender_name, m.id
                )
            }
            ActorEvent::MessageRevoked { message, .. } => {
                format!(
                    "MessageRevoked(sender_id={}, sender_name={}, mid={})",
                    ctx.sender_id, ctx.sender_name, message
                )
            }
            ActorEvent::MessageReclassified { message, classification } => {
                format!(
                    "MessageReclassified(sender_id={}, sender_name={}, mid={}, classification={})",
                    ctx.sender_id, ctx.sender_name, message, classification
                )
            }
            ActorEvent::RetryRequested { target, author } => {
                format!(
                    "RetryRequested(sender_id={}, {}, author={})",
                    ctx.sender_id, target, author
                )
            }
            ActorEvent::RetryAccepted { target } => {
                format!("RetryAccepted(sender_id={}, {})", ctx.sender_id, target)
            }
            ActorEvent::RetryRejected { target, reason } => {
                format!(
                    "RetryRejected(sender_id={}, {}, reason={:?})",
                    ctx.sender_id, target, reason
                )
            }
            ActorEvent::MessageRejected { rejected, message, .. } => {
                format!(
                    "MessageRejected(sender_id={}, rejected={}, mid={:?})",
                    ctx.sender_id, rejected, message
                )
            }
            _ => {
                format!("{:?}", self.event)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorState, Rejection, RequestReason, RetryTarget};
    use crate::agent::{AgentId, RetryRefused};
    use crate::approval::ApproverId;
    use crate::confidentiality::Classification;
    use crate::message::testing::{id, make_message};
    use crate::message::{Message, MessageId, MessageRole};
    use crate::question::QuestionId;
    use crate::turn::GivenUpReason;
    use crate::user::UserId;
    use crate::work::TurnId;
    use std::collections::HashSet;

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
            Self {
                agents: [agent(), agent()],
                users: [user(), user()],
                approver: ActorId::Approver(ApproverId::new()),
            }
        }

        /// Whom of them `event` of `sender` goes to (`Envelope::recipients`).
        fn concerned(&self, sender: ActorId, event: ActorEvent) -> HashSet<ActorId> {
            let envelope =
                Envelope::new(EventContext { sender_id: sender, ..EventContext::default() }, event);
            let recipients = envelope.recipients();
            self.agents
                .iter()
                .chain(&self.users)
                .chain([&self.approver])
                .copied()
                .filter(|id| recipients.names(*id))
                .collect()
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
        let said = |content: Option<&str>, role: MessageRole| Message {
            sender_id: sender,
            role,
            content: content.map(str::to_string),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let call = ToolCall {
            id: Some("1".to_string()),
            function_name: "noop".to_string(),
            arguments: serde_json::json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        };
        let tool = Message {
            tool_results: vec![ToolCallResult::of(
                &call,
                Default::default(),
                "done".to_string(),
                Classification::CHANNEL_DEFAULT,
            )],
            ..said(Some("done"), MessageRole::Tool)
        };

        assert_eq!(
            present.concerned(
                sender,
                ActorEvent::CompleteMessageGenerated(said(Some("hello"), MessageRole::Assistant))
            ),
            present.users_and_agents(),
            "words, to everyone"
        );
        for unheard in [
            tool,
            said(None, MessageRole::Assistant),
            Message { internal: true, ..said(Some("thinking"), MessageRole::Assistant) },
        ] {
            assert_eq!(
                present.concerned(sender, ActorEvent::CompleteMessageGenerated(unheard)),
                present.users(),
                "nothing to hear, the users alone"
            );
        }
        let id = MessageId::new();
        for changed in [
            ActorEvent::MessageModified(said(Some("changed"), MessageRole::Assistant)),
            ActorEvent::MessageRevoked { message: id, at: Utc::now() },
            ActorEvent::MessageReclassified { message: id, classification: Classification::PUBLIC },
            ActorEvent::ParticipantJoined {
                participant: present.users[1],
                name: "bob".to_string(),
            },
            ActorEvent::ParticipantLeft {
                participant: present.agents[1],
                name: "helper".to_string(),
                until: Position::new(0),
            },
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
        let turn_id = TurnId::new();
        let message = Message::new(Classification::CHANNEL_DEFAULT);
        let step = |index| (index, 2, "first".to_string());
        let target = RetryTarget::Turn(turn_id);
        for event in [
            ActorEvent::ResponseStarted { turn_id },
            ActorEvent::StateChanged(ActorState::Planning),
            ActorEvent::PartialMessageGenerated(message.clone()),
            ActorEvent::IntermediatePartialGenerated(message.clone()),
            ActorEvent::IntermediateMessageGenerated(message.clone()),
            ActorEvent::PlanStepStarted {
                turn_id,
                step_index: step(0).0,
                total_steps: step(0).1,
                description: step(0).2,
            },
            ActorEvent::PlanStepFinished {
                turn_id,
                step_index: step(0).0,
                total_steps: step(0).1,
                description: step(0).2,
            },
            ActorEvent::TurnFinished { turn_id, messages: vec![message.clone()] },
            ActorEvent::TurnGivenUp {
                turn_id,
                reason: GivenUpReason::Failed { message: "boom".to_string() },
            },
            ActorEvent::ActivityStarted { turn_id, activity: "compacting".to_string() },
            ActorEvent::ActivityFinished {
                turn_id,
                activity: "compacting".to_string(),
                output: "done".to_string(),
            },
            ActorEvent::RetryAccepted { target },
            ActorEvent::RetryRejected { target, reason: RetryRefused::NotLast },
        ] {
            let kind = event.kind();
            assert_eq!(
                present.concerned(present.agents[0], event),
                present.users(),
                "{kind} goes to the users, not to the agents"
            );
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
        let command = |to| ActorEvent::CommandExecuted {
            command: "wakeup".to_string(),
            args: String::new(),
            output: "Woke b.".to_string(),
            to,
            turn_id,
        };

        assert_eq!(
            present.concerned(
                issuer,
                ActorEvent::CommandRequested {
                    command: "compact".to_string(),
                    args: String::new(),
                    agent
                }
            ),
            HashSet::from([agent])
        );
        assert_eq!(
            present.concerned(
                issuer,
                ActorEvent::RetryRequested {
                    target: RetryTarget::Message(MessageId::new()),
                    author: agent
                }
            ),
            HashSet::from([agent])
        );
        assert_eq!(
            present.concerned(
                issuer,
                ActorEvent::CommandRequested {
                    command: "compact".to_string(),
                    args: String::new(),
                    agent: issuer
                }
            ),
            HashSet::new(),
            "only an agent carries one out"
        );
        assert_eq!(
            present.concerned(agent, command(None)),
            present.users(),
            "everybody's answer, to the users"
        );
        assert_eq!(
            present.concerned(agent, command(Some(issuer))),
            HashSet::from([issuer]),
            "a private one, to its issuer"
        );
        assert_eq!(
            present.concerned(
                agent,
                ActorEvent::CommandStarted {
                    command: "compact".to_string(),
                    args: String::new(),
                    to: issuer,
                    turn_id: TurnId::new()
                }
            ),
            HashSet::from([issuer])
        );
        assert_eq!(
            present.concerned(
                agent,
                ActorEvent::CommandFailed {
                    command: "compact".to_string(),
                    reason: "no".to_string(),
                    to: issuer,
                    turn_id
                }
            ),
            HashSet::from([issuer])
        );
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
        let request = || {
            ActorEvent::ApprovalRequested(ApprovalRequest {
                suggested: None,
                explicit: false,
                id: Default::default(),
                session_id: Default::default(),
                channel_id: Default::default(),
                agent_id,
                tool_call: ToolCall {
                    id: None,
                    function_name: "noop".to_string(),
                    arguments: serde_json::json!({}),
                    visibility: Default::default(),
                    presentation: Default::default(),
                },
                description: "test".to_string(),
            })
        };
        let withdrawn = || ActorEvent::ApprovalRequestWithdrawn { id: Default::default() };
        let response = || ApprovalResponse::new(Default::default(), agent_id);

        for asked in [request(), withdrawn()] {
            let kind = asked.kind();
            assert_eq!(
                present.concerned(agent, asked),
                HashSet::from([approver]),
                "{kind}: internal, not shown to the users yet"
            );
        }
        for asked in [request(), withdrawn()] {
            assert_eq!(
                present.concerned(approver, asked),
                present.users(),
                "what the users act on, or no longer"
            );
        }
        assert!(
            present.concerned(user, withdrawn()).is_empty(),
            "only an agent and the approver withdraw"
        );
        assert_eq!(
            present.concerned(user, ActorEvent::ApprovalGranted(response())),
            HashSet::from([approver]),
            "a user's answer, to the approver alone"
        );
        assert!(
            present.concerned(user, ActorEvent::ApprovalTimedOut(response())).is_empty(),
            "a timeout is the approver's alone"
        );
        let decided = present.users().into_iter().chain([agent]).collect::<HashSet<_>>();
        for decision in [
            ActorEvent::ApprovalGranted(response()),
            ActorEvent::ApprovalDenied(response()),
            ActorEvent::ApprovalTimedOut(response()),
        ] {
            let kind = decision.kind();
            assert_eq!(
                present.concerned(approver, decision),
                decided,
                "{kind} ends the approval for the users, and goes to the agent that asked"
            );
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
        let request = |sensitive| {
            ActorEvent::QuestionRequested(QuestionRequest {
                id: QuestionId::new(),
                session_id: Default::default(),
                channel_id: Default::default(),
                agent_id,
                tool_call: ToolCall {
                    id: None,
                    function_name: "noop".to_string(),
                    arguments: serde_json::json!({}),
                    visibility: Default::default(),
                    presentation: Default::default(),
                },
                questions: Questions {
                    questions: vec![Question {
                        header: "Name".to_string(),
                        question: "Who?".to_string(),
                        choices: Vec::new(),
                        multi_select: false,
                    }],
                    ask: vec![Addressee::Actor(alice)],
                    sensitive,
                },
            })
        };
        let closed = |shown_to| ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to };

        assert_eq!(
            present.concerned(agent, request(false)),
            present.users(),
            "every user sees what the agent waits for"
        );
        assert_eq!(
            present.concerned(agent, request(true)),
            HashSet::from([alice]),
            "sensitive: those asked alone"
        );
        assert!(present.concerned(alice, request(false)).is_empty(), "only an agent asks");
        assert_eq!(present.concerned(agent, closed(None)), present.users());
        assert_eq!(present.concerned(agent, closed(Some(vec![alice]))), HashSet::from([alice]));
        let answered =
            ActorEvent::QuestionAnswered { id: QuestionId::new(), agent_id, responses: Vec::new() };
        assert_eq!(
            present.concerned(alice, answered.clone()),
            HashSet::from([agent]),
            "neither the other agent, the other users nor the approver"
        );
        assert_eq!(
            present
                .concerned(alice, ActorEvent::QuestionDeclined { id: QuestionId::new(), agent_id }),
            HashSet::from([agent])
        );
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
            ActorEvent::ResponseRequested { reason: RequestReason::Chosen },
            ActorEvent::WakeupRequested,
            ActorEvent::NudgeRequested { participant: present.agents[0] },
            ActorEvent::ChainCutOff {
                message: MessageId::new(),
                max_agent_hops: 3,
                not_asked: Vec::new(),
            },
            ActorEvent::ResponderUndecided { message: MessageId::new() },
            ActorEvent::MessageRejected {
                rejected: "CompleteMessageGenerated".to_string(),
                message: None,
                reason: Rejection::Forbidden,
            },
        ] {
            let kind = event.kind();
            assert!(present.concerned(user, event).is_empty(), "{kind}");
        }
    }

    /// Issue 223: of what the history does not record, the answers to
    /// questions are not lost on the way; the rest is transient.
    #[test]
    fn of_what_is_not_recorded_only_the_answers_are_kept() {
        let turn_id = TurnId::new();
        let message = make_message(id(1));
        for (event, transient) in [
            (
                ActorEvent::QuestionAnswered {
                    id: QuestionId::new(),
                    agent_id: AgentId::new(),
                    responses: Vec::new(),
                },
                false,
            ),
            (
                ActorEvent::QuestionDeclined { id: QuestionId::new(), agent_id: AgentId::new() },
                false,
            ),
            (ActorEvent::PartialMessageGenerated(message.clone()), true),
            (ActorEvent::IntermediatePartialGenerated(message), true),
            (ActorEvent::StateChanged(ActorState::Idle), true),
            (ActorEvent::ResponseStarted { turn_id }, true),
            (
                ActorEvent::CommandStarted {
                    command: "mark".to_string(),
                    args: String::new(),
                    to: ActorId::None,
                    turn_id,
                },
                true,
            ),
            (ActorEvent::ActivityStarted { turn_id, activity: "compacting".to_string() }, true),
            (
                ActorEvent::PlanStepFinished {
                    turn_id,
                    step_index: 0,
                    total_steps: 1,
                    description: "first".to_string(),
                },
                true,
            ),
            (ActorEvent::WakeupRequested, true),
            (ActorEvent::NudgeRequested { participant: ActorId::None }, true),
        ] {
            let kind = event.kind();
            assert!(
                Envelope::new(EventContext::default(), event.clone()).entry().is_none(),
                "{kind} is not recorded"
            );
            assert_eq!(event.is_transient(), transient, "{kind}");
        }
    }
}
