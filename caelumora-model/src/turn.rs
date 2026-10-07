//! The working state of an agent's turn (issue 85): what one turn of an
//! agent run here is and what it has produced and staged so far, stored with
//! the agent's `AgentChannelState` while it runs. What a turn does to that
//! state when it begins and ends are rules of the state
//! (`AgentChannelState::begin_turn`, `record_turn`, `give_up_turn`).

use crate::actor::{ActorId, ActorState};
use crate::approval::ApprovalRequest;
use crate::chain::Node;
use crate::context::ContextRef;
use crate::memory::{MemoryChange, MemoryOrigin};
use crate::message::Message;
use crate::note::NoteChange;
use crate::plan::PlanState;
use crate::tool::{Tool, ToolCallState, ToolCallStatus};
use crate::transcript::TranscriptEntry;
use crate::work::Work;
use serde::{Deserialize, Serialize};

/// The working state of one turn (issue 85): what the turn is - fixed when
/// it begins - and what it has produced and staged so far, none of it taken
/// over yet. Taking the turn over (`Turn::commit`) or giving it up
/// (`Turn::give_up`, issue 145) ends it as a whole; a branch switch (issue
/// 08) will give it up the same way.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// What the turn is done in and as, what it has seen, and what its
    /// next message is produced from (`Work::context`): the transcript so
    /// far - the agent's, continued by the turn's own messages - the prompt
    /// parts placed in it, the tools offered, the knowledge read, the rules
    /// of the model. Resolved by `crate::context::ContextResolver`; what is
    /// recorded with each message. Stored in line, as the turn's own fields
    /// were before work was one type.
    #[serde(flatten)]
    pub work: Work,
    /// The agent's tools as they were when the turn began (issue 137): what
    /// its model is offered for every call of the turn, and what its calls
    /// are labelled and put up for approval by - one set for the whole turn,
    /// a resumed one included.
    pub tools: Vec<Tool>,
    /// The messages the turn produced so far, each with the digest it will
    /// have in the transcript and what it was produced from.
    pub entries: Vec<TurnEntry>,
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
                ToolCallStatus::WaitingApproval { id, description, explicit, suggested } => Some(ApprovalRequest {
                    id: *id,
                    session_id: self.work.event_context.session_id,
                    channel_id: self.work.event_context.channel_id,
                    agent_id: agent,
                    tool_call: c.tool_call.clone(),
                    description: description.clone(),
                    explicit: *explicit,
                    suggested: Some(suggested.clone()),
                }),
                _ => None,
            })
            .collect()
    }

    /// Appends `message`, produced by the turn from its context as it
    /// stands, and moves the context on past it (issue 136). The message
    /// gets the digest it will have in the transcript, computed as the
    /// transcript's chain would: the turn's messages continue the agent's
    /// transcript, which does not change while a turn runs. It is the
    /// turn's from now on (`Message::turn_id`, issue 235), whoever built
    /// it.
    pub fn push(&mut self, mut message: Message) {
        message.turn_id = Some(self.work.turn_id);
        let context = self.work.context.clone();
        let digest = crate::digest::message_chain(context.transcript.as_ref(), &message);
        self.work.context.transcript = Some(digest);
        self.entries.push(TurnEntry { node: Node::new(digest, context.transcript, TranscriptEntry::Message(message)), context });
    }

    /// The messages the turn produced so far.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter().filter_map(|e| e.node.entry.message())
    }
}

/// A message a turn produced, not yet taken over (issue 136): as it will be
/// in the transcript, and what it was produced from - what is recorded with
/// it in the history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnEntry {
    pub node: Node<TranscriptEntry>,
    pub context: ContextRef,
}
