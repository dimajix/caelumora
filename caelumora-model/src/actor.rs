use crate::agent::AgentId;
use crate::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApproverId};
use crate::channel::ChannelId;
use crate::message::{Message, MessageId};
use crate::security::SecurityContext;
use crate::session::SessionId;
use crate::user::UserId;
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
    ResponseRequested,
    ResponseStarted,
    PartialMessageGenerated(Message),
    CompleteMessageGenerated(Message),
    FullResponseGenerated(Vec<Message>),
    WakeupRequested,
    /// Streaming text produced by the agent while it is not yet ready to give
    /// its final answer - still deliberating (researching, proposing a plan)
    /// or executing an already-approved plan. Never the final response.
    IntermediatePartialGenerated(Message),
    /// A completed message that still carries tool calls (or proposes a
    /// plan), produced while deliberating or executing an approved plan -
    /// not the final answer.
    IntermediateMessageGenerated(Message),
    /// A plan step has just begun execution. Emitted directly by `AgentActor`
    /// whenever it pushes that step's instruction, so observers (e.g. the web
    /// UI) can show which step of the plan is currently in progress.
    PlanStepStarted {
        step_index: usize,
        total_steps: usize,
        description: String,
    },
    /// The plan step that was in progress has converged. Emitted directly by
    /// `AgentActor` right before it advances to the next step (or to
    /// finalization).
    PlanStepFinished {
        step_index: usize,
        total_steps: usize,
        description: String,
    },
    /// The agent's turn was aborted by an error, e.g. an unreachable LLM
    /// backend or a failing component. The agent is idle again and takes the
    /// next request. `message` is meant for users: it names what failed (a
    /// component, an endpoint) but carries no file paths or secret values;
    /// the full error is in the log under the envelope's `span_id`.
    ///
    /// Not part of the conversation: it is shown to users but never written
    /// to the history, so it does not end up in a model's context.
    TurnFailed {
        message: String,
    },
    /// A user ran a command whose answer concerns everybody in the channel,
    /// e.g. `/wakeup` (issue 28). The envelope's sender is the user who ran
    /// it; `command` is its name without the slash, `output` what it answered.
    ///
    /// Like `TurnFailed`, shown to users and never written to the history:
    /// commands address the system, not the agents.
    CommandExecuted {
        command: String,
        output: String,
    },
    /// `participant` was removed from the channel (issue 162). The envelope's
    /// sender is who removed it - the runtime itself, `ActorId::System`, if
    /// nobody did. Sent by the channel once the participant's actor no longer
    /// runs in it: nothing of the participant follows, so whatever of it was
    /// under way - a turn, a call of a tool - has ended.
    ///
    /// Like `TurnFailed`, shown to users and never written to the history.
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
    /// Like `TurnFailed`, never written to the history; an agent takes no
    /// note of it.
    ChainCutOff {
        message: MessageId,
        max_agent_hops: u32,
        not_asked: Vec<ActorId>,
    },
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
            ActorEvent::FullResponseGenerated(messages) => Some(ActorEvent::FullResponseGenerated(
                messages.iter().filter_map(Message::published).collect(),
            )),
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
            | ActorEvent::IntermediateMessageGenerated(m) => vec![m],
            ActorEvent::FullResponseGenerated(messages) => messages.iter().collect(),
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
            ActorEvent::ResponseRequested => "ResponseRequested",
            ActorEvent::ResponseStarted => "ResponseStarted",
            ActorEvent::PartialMessageGenerated(_) => "PartialMessageGenerated",
            ActorEvent::CompleteMessageGenerated(_) => "CompleteMessageGenerated",
            ActorEvent::FullResponseGenerated(_) => "FullResponseGenerated",
            ActorEvent::WakeupRequested => "WakeupRequested",
            ActorEvent::IntermediatePartialGenerated(_) => "IntermediatePartialGenerated",
            ActorEvent::IntermediateMessageGenerated(_) => "IntermediateMessageGenerated",
            ActorEvent::PlanStepStarted { .. } => "PlanStepStarted",
            ActorEvent::PlanStepFinished { .. } => "PlanStepFinished",
            ActorEvent::TurnFailed { .. } => "TurnFailed",
            ActorEvent::CommandExecuted { .. } => "CommandExecuted",
            ActorEvent::ParticipantRemoved { .. } => "ParticipantRemoved",
            ActorEvent::ChainCutOff { .. } => "ChainCutOff",
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
    pub fn new(session_id: SessionId, channel_id: ChannelId, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id,
            channel_id,
            sender_id,
            sender_name,
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::default(),
            span_id: Uuid::new_v4(),
            span_depth: 0,
            agent_hops: 0,
        }
    }

    pub fn next(&self, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            sender_id,
            sender_name,
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
            ActorEvent::FullResponseGenerated(v) => match v.last() {
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
            ActorEvent::TurnFailed { message } => {
                format!("TurnFailed(sender_id={}, sender_name={}, message={})", ctx.sender_id, ctx.sender_name, message)
            }
            ActorEvent::CommandExecuted { command, .. } => {
                format!("CommandExecuted(sender_id={}, sender_name={}, command={})", ctx.sender_id, ctx.sender_name, command)
            }
            ActorEvent::ParticipantRemoved { participant } => {
                format!("ParticipantRemoved(sender_id={}, sender_name={}, participant={})", ctx.sender_id, ctx.sender_name, participant)
            }
            ActorEvent::ChainCutOff { message, max_agent_hops, not_asked } => {
                format!("ChainCutOff(sender_id={}, message={}, max_agent_hops={}, not_asked={})", ctx.sender_id, message, max_agent_hops, not_asked.len())
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
    WaitingForApproval,
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
        ActorState::WaitingForApproval,
    ];

    /// Whether the state belongs to a turn - an error in it ends the turn,
    /// not the actor - rather than waiting between turns.
    pub fn in_turn(self) -> bool {
        !matches!(self, ActorState::Idle | ActorState::Sleeping)
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
    fn every_state_is_kept_as_its_own_byte() {
        for state in ActorState::ALL {
            assert_eq!(ActorState::from_raw(state.raw()), state);
        }
        let holder = ProcessingStateHolder::new(ActorState::Sleeping);
        assert!(!holder.replace(ActorState::Planning, ActorState::Idle), "not in that state");
        assert!(holder.replace(ActorState::Sleeping, ActorState::Idle));
        assert_eq!(holder.load(), ActorState::Idle);
        assert!(ActorState::WaitingForApproval.in_turn() && !ActorState::Sleeping.in_turn());
    }

    #[test]
    fn published_events_carry_redacted_messages() {
        use crate::tool::{ToolCall, ToolVisibility};
        let hidden = Message {
            tool_calls: vec![ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN }],
            ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let text = Message { content: Some("hi".to_string()), ..Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT) };

        assert!(ActorEvent::CompleteMessageGenerated(hidden.clone()).published().is_none(), "nothing left to show");
        assert!(ActorEvent::PartialMessageGenerated(hidden.clone()).published().is_none());
        match ActorEvent::FullResponseGenerated(vec![hidden.clone(), text.clone()]).published() {
            Some(ActorEvent::FullResponseGenerated(messages)) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].id, text.id);
            }
            other => panic!("expected the response, got {other:?}"),
        }
        assert!(
            matches!(ActorEvent::FullResponseGenerated(vec![hidden]).published(), Some(ActorEvent::FullResponseGenerated(m)) if m.is_empty()),
            "the end of the turn is still announced"
        );
    }
}
