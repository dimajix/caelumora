//! The working state of an agent's turn (issue 85): what one turn of an
//! agent run here is and what it has produced and staged so far, stored with
//! the agent's `AgentChannelState` while it runs. What a turn does to that
//! state when it begins, says something and ends are rules of the state
//! (`AgentChannelState::begin_turn`, `said`, `finish_turn`, `give_up_turn`).
//! What it says is not kept here: it goes into the agent's transcript and
//! history as it is said (issue 260).

use crate::actor::{ActorId, ActorState};
use crate::approval::ApprovalRequest;
use crate::question::QuestionRequest;
use crate::memory::{MemoryChange, MemoryOrigin};
use crate::note::NoteChange;
use crate::plan::PlanState;
use crate::tool::{ToolCallState, ToolCallStatus};
use crate::confidentiality::Classification;
use crate::context::KnowledgeRef;
use crate::message::Message;
use crate::work::Work;
use serde::{Deserialize, Serialize};

/// Why a turn was given up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GivenUpReason {
    /// It failed; `message` is what its users were told.
    Failed { message: String },
    /// The channel's classification was lowered to `channel` while the
    /// turn ran (issue 141): what the turn saw may not flow into it any
    /// more.
    Lowered { channel: Classification },
    /// The agent left the channel: removed from it, or the channel or its
    /// session deleted (issue 377). Who did that is a matter of that act,
    /// not of its consequence - the audit log's (issue 140). An agent whose
    /// session merely stops keeps its turn, to be resumed.
    Left,
    /// Its answer was asked for again while it was under way (issue 235):
    /// a turn of its own answers instead.
    Retried,
    /// It asked its model `calls` times, as often as a turn may, and was
    /// not done (issue 411): the limit is a safety net against a model that
    /// does not come to an end, so reaching it is no answer.
    CallLimit { calls: u32 },
}

impl GivenUpReason {
    /// Whether the agent may be asked to answer anew after a turn given up
    /// so (issue 259): one that failed, whose channel was lowered below it
    /// or that reached the limit of its model calls (issue 411). One asked
    /// for again is answered anew already; one given up as it left belongs
    /// to an agent that left.
    pub fn may_be_answered_again(&self) -> bool {
        matches!(self, GivenUpReason::Failed { .. } | GivenUpReason::Lowered { .. } | GivenUpReason::CallLimit { .. })
    }

    /// Why the turn was given up, as its agent is told: "… because it
    /// failed". A fixed text, which says nothing of the conversation.
    pub fn cause(&self) -> &'static str {
        match self {
            GivenUpReason::Failed { .. } => "it failed",
            GivenUpReason::Lowered { .. } => "the channel's classification was lowered",
            GivenUpReason::Left => "you left the channel",
            GivenUpReason::Retried => "your answer was asked for again",
            GivenUpReason::CallLimit { .. } => "it asked the model as often as one turn may",
        }
    }
}

/// The working state of one turn (issue 85): what the turn is - fixed when
/// it begins - and what it has staged so far, none of it taken over yet;
/// what it said is in the transcript already (issue 260). Taking the turn over (`Turn::commit`) or giving it up
/// (`Turn::give_up`, issue 145) ends it as a whole; a branch switch (issue
/// 08) will give it up the same way.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// What the turn is done in and as, what it has seen, and what its
    /// next message is produced from (`Work::context`): the transcript so
    /// far - its head, the turn's own messages included - the prompt parts
    /// placed in it, the tools offered, the knowledge read, the rules
    /// of the model. Resolved by `crate::context::ContextResolver`; what is
    /// recorded with each message. Stored in line, as the turn's own fields
    /// were before work was one type.
    #[serde(flatten)]
    pub work: Work,
    /// Changes to the agent's memory, each with where it was made, applied
    /// to the agent's `AgentState` as it is when the turn completes (issue
    /// 62).
    pub memory: Vec<(MemoryChange, MemoryOrigin)>,
    /// Changes to the agent's notes in this channel, applied when the turn
    /// completes (issue 65).
    pub notes: Vec<NoteChange>,
    /// Changes to its private notes in this channel, likewise (issue 51).
    pub private_notes: Vec<NoteChange>,
    /// The tool calls of the current step, with their approvals and results.
    pub tool_calls: Vec<ToolCallState>,
    /// What the turn was given to read while the calls of its step ran
    /// (issue 389), each with the knowledge it is, if it is knowledge:
    /// written after the step's tool message (`AgentChannelState::said`),
    /// since nothing may stand between calls and their results.
    pub given_after_step: Vec<(Message, Option<KnowledgeRef>)>,
    /// The plan in progress, if any.
    pub plan: Option<PlanState>,
    /// Set by a tool (the stop tool): the turn ends with this tool step, and
    /// the agent lands in this state instead of `Idle`.
    pub requested_state: Option<ActorState>,
    /// How many times the turn asked its model so far - kept with the turn,
    /// so that its limit holds for a turn resumed after a restart too
    /// (issue 157).
    #[serde(default)]
    pub model_calls: u32,
    /// How many calls the turn had made when it last tried to compact its
    /// context (issue 284): it tries once before each call, not again when
    /// there was nothing to compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_tried_at: Option<u32>,
    /// The command the turn carries out (issue 269), `None` for a turn
    /// that answers. A command turn has no tools and no context of its
    /// own; one resumed after a restart runs its command again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandRequest>,
}

/// A command an agent was asked to carry out (issue 269), as the turn
/// keeps it: the command's name without the slash, its arguments as they
/// came - without the handle that named the agent -, and who asked, the
/// request's initiator, kept here so that the record of the turn's end
/// needs nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    pub args: String,
    pub issuer: ActorId,
}

impl TurnState {
    /// The approval requests of the calls of the current step that still
    /// wait for a decision, as the turn asks for them - the same ids every
    /// time, in the name of the agent the turn's context is sent by. The
    /// turn is their record (issue 172): the approver is asked again from
    /// it when the turn resumes, and a client is shown them from it. None
    /// for a turn whose context is sent by no agent.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        let ActorId::Agent(agent) = self.work.event_context.sender_id else { return Vec::new() };
        self.tool_calls.iter()
            .filter_map(|c| match &c.status {
                ToolCallStatus::WaitingApproval { id, description, explicit, suggested, .. } => Some(ApprovalRequest {
                    id: *id,
                    session_id: self.work.event_context.session_id,
                    channel_id: self.work.event_context.channel_id,
                    agent_id: agent,
                    tool_call: c.tool_call.clone(),
                    description: description.clone(),
                    explicit: *explicit,
                    suggested: suggested.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// The questions the calls of the current step still wait for the
    /// answers to (issue 165), as the turn asks them - the same ids every
    /// time, in the name of the agent the turn's context is sent by. The
    /// turn is their record, as for approvals. None for a turn whose
    /// context is sent by no agent.
    pub fn pending_questions(&self) -> Vec<QuestionRequest> {
        let ActorId::Agent(agent) = self.work.event_context.sender_id else { return Vec::new() };
        self.tool_calls.iter()
            .filter_map(|c| match &c.status {
                ToolCallStatus::WaitingUser { id, questions } => Some(QuestionRequest {
                    id: *id,
                    session_id: self.work.event_context.session_id,
                    channel_id: self.work.event_context.channel_id,
                    agent_id: agent,
                    tool_call: c.tool_call.clone(),
                    questions: questions.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}
