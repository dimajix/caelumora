use crate::agent::AgentId;
use crate::approval::{ApprovalRequest, ApprovalResponse};
use crate::channel::ChannelId;
use crate::message::{Message, MessageEvent};
use crate::security::SecurityContext;
use crate::session::SessionId;
use crate::user::UserId;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorId {
    #[default]
    None,
    User(UserId),
    Agent(AgentId),
    Approver(Uuid),
    /// The runtime itself, acting on no one's behalf: setting up initial
    /// data, keeping its own records in step with events. Only ever set
    /// deliberately, through `SecurityContext::system()` - never the default,
    /// so that a context nobody filled in has no rights (issue 11).
    System,
}
impl ActorId {
    pub fn id(&self) -> Uuid {
        match &self {
            ActorId::None => {Uuid::nil()}
            ActorId::User(id) => {id.0}
            ActorId::Agent(id) => {id.0}
            ActorId::Approver(id) => {*id}
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
}

impl ActorEvent {
    /// The event as it leaves its sender for the channel (issues 54, 61): the
    /// messages it carries redacted by the labels `may_publish` refuses
    /// (`Message::published`). `None` if it carried a single message of which
    /// nothing is left to show. Approval requests are left as they are: the
    /// approvers see the arguments of a call they are asked to decide.
    pub fn published(self, may_publish: &dyn Fn(crate::confidentiality::Confidentiality) -> bool) -> Option<ActorEvent> {
        let publish = |message: Message, wrap: fn(Message) -> ActorEvent| message.published(may_publish).map(wrap);
        match self {
            ActorEvent::PartialMessageGenerated(m) => publish(m, ActorEvent::PartialMessageGenerated),
            ActorEvent::CompleteMessageGenerated(m) => publish(m, ActorEvent::CompleteMessageGenerated),
            ActorEvent::IntermediatePartialGenerated(m) => publish(m, ActorEvent::IntermediatePartialGenerated),
            ActorEvent::IntermediateMessageGenerated(m) => publish(m, ActorEvent::IntermediateMessageGenerated),
            ActorEvent::FullResponseGenerated(messages) => Some(ActorEvent::FullResponseGenerated(
                messages.iter().filter_map(|m| m.published(may_publish)).collect(),
            )),
            other => Some(other),
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
        }
    }

    pub fn from_message_event(event: MessageEvent) -> Self {
        match event {
            MessageEvent::Partial(msg) => {
                ActorEvent::PartialMessageGenerated(msg)
            }
            MessageEvent::Complete(msg) => {
                ActorEvent::CompleteMessageGenerated(msg)
            }
            MessageEvent::EndOfTurn(vec) => {
                ActorEvent::FullResponseGenerated(vec)
            }
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub event_id: Uuid,
    pub correlation_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
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

    pub fn forward(&self) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: self.event.clone(),
        }
    }
    pub fn forward_with_response(&self, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }
    pub fn forward_with_event(&self, event:ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event
        }
    }

    pub fn respond(&self, sender_id:ActorId, sender_name:String, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.next(sender_id),
            context: self.context.next(sender_id, sender_name),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }

    pub fn with_event(mut self, event: ActorEvent) -> Self {
        self.event = event;
        self
    }
    pub fn with_security(mut self, security_context: SecurityContext) -> Self {
        self.security = security_context;
        self
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
            ActorEvent::FullResponseGenerated(v) => {
                let default_message = Message::default();
                let m = v.last().unwrap_or(&default_message);
                format!("EndOfTurn(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
            }
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
            _ => {
                format!("{:?}", self.event)
            }
        }
    }
}


#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Stop current turn
    Stopping,
    WaitingForApproval,
}

impl ActorState {
    /// The state behind the byte `ProcessingStateHolder` stores.
    fn from_raw(v: u8) -> Self {
        match v {
            0 => ActorState::Idle,
            2 => ActorState::Cancelling,
            3 => ActorState::Sleeping,
            4 => ActorState::Stopping,
            5 => ActorState::WaitingForApproval,
            6 => ActorState::Planning,
            7 => ActorState::ExecutingTools,
            8 => ActorState::Reflecting,
            _ => ActorState::Idle,
        }
    }

    fn raw(self) -> u8 {
        match self {
            ActorState::Idle => 0,
            ActorState::Cancelling => 2,
            ActorState::Sleeping => 3,
            ActorState::Stopping => 4,
            ActorState::WaitingForApproval => 5,
            ActorState::Planning => 6,
            ActorState::ExecutingTools => 7,
            ActorState::Reflecting => 8,
        }
    }
}

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

    /// The core "CAS Loop" logic.
    /// This takes a closure that decides the next state based on the current one.
    pub fn transition<F>(&self, decision_logic: F) -> ActorState
    where
        F: Fn(ActorState) -> Option<ActorState>,
    {
        let mut current_raw = self.state.load(Ordering::SeqCst);
        loop {
            let current_state = ActorState::from_raw(current_raw);

            // 1. Execute the decision logic provided by the caller
            // We return Option: Some(new_state) to transition, None to do nothing.
            let next_state = match decision_logic(current_state) {
                Some(s) => s,
                None => return current_state, // No transition needed
            };

            // 2. Try to atomicaly swap the value.
            // compare_exchange checks if the value is still 'current_raw'.
            // If yes, it sets it to 'next_state' and returns Ok.
            // If no, it returns Err(actual_value_now_in_memory).
            match self.state.compare_exchange(
                current_raw,
                next_state.raw(),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    // Success! The state transitioned without interference.
                    return next_state;
                }
                Err(actual_new_value) => {
                    // Failure! Someone else changed the state.
                    // Update our 'current_raw' and retry the loop with the new reality.
                    current_raw = actual_new_value;
                }
            }
        }
    }}


// We use this helper to bridge u8 <-> ProcessingState during Serde operations
pub mod processing_state_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    // Serialization: Arc<ProcessingStateHolder> -> ProcessingState (String/Enum)
    pub fn serialize<S>(val: &Arc<ProcessingStateHolder>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let state = val.load();
        state.serialize(serializer)
    }

    // Deserialization: ProcessingState (String/Enum) -> Arc<AtomicU8>
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<ProcessingStateHolder>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let state: ActorState = Deserialize::deserialize(deserializer)?;
        Ok(Arc::new(ProcessingStateHolder::new(state)))
    }
}

pub fn default_processing_state() -> Arc<ProcessingStateHolder> {
    Arc::new(ProcessingStateHolder::new(ActorState::Idle))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorState;

    #[test]
    fn test_processing_state_conversion() {
        assert_eq!(ActorState::Idle.raw(), 0);
        assert_eq!(ActorState::WaitingForApproval.raw(), 5);
        assert_eq!(ActorState::from_raw(5), ActorState::WaitingForApproval);
        assert_eq!(ActorState::Planning.raw(), 6);
        assert_eq!(ActorState::ExecutingTools.raw(), 7);
        assert_eq!(ActorState::Reflecting.raw(), 8);
        assert_eq!(ActorState::from_raw(6), ActorState::Planning);
        assert_eq!(ActorState::from_raw(7), ActorState::ExecutingTools);
        assert_eq!(ActorState::from_raw(8), ActorState::Reflecting);
    }

    #[test]
    fn published_events_carry_redacted_messages() {
        use crate::confidentiality::Confidentiality;
        use crate::tool::{ToolCall, ToolConfidentiality};
        let hidden = Message {
            tool_calls: vec![ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), confidentiality: ToolConfidentiality::HIDDEN }],
            ..Message::default()
        };
        let text = Message { content: Some("hi".to_string()), ..Message::default() };
        let public_only = |label: Confidentiality| label == Confidentiality::Public;

        assert!(ActorEvent::CompleteMessageGenerated(hidden.clone()).published(&public_only).is_none(), "nothing left to show");
        assert!(ActorEvent::PartialMessageGenerated(hidden.clone()).published(&public_only).is_none());
        match ActorEvent::FullResponseGenerated(vec![hidden.clone(), text.clone()]).published(&public_only) {
            Some(ActorEvent::FullResponseGenerated(messages)) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].id, text.id);
            }
            other => panic!("expected the response, got {other:?}"),
        }
        assert!(
            matches!(ActorEvent::FullResponseGenerated(vec![hidden]).published(&public_only), Some(ActorEvent::FullResponseGenerated(m)) if m.is_empty()),
            "the end of the turn is still announced"
        );
    }
}
