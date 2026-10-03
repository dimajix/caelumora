//! The working state of an agent's turn (issue 85): what one turn of an
//! agent run here is and what it has produced and staged so far, stored with
//! the agent's `AgentChannelState` while it runs. What a turn does to that
//! state when it begins and ends are rules of the state
//! (`AgentChannelState::begin_turn`, `record_turn`, `give_up_turn`).

use crate::actor::{ActorId, ActorState, EventContext};
use crate::approval::{ApprovalRequest, ApprovalStatus};
use crate::chain::Node;
use crate::confidentiality::Classification;
use crate::context::{ContextRef, KnowledgeRef};
use crate::memory::{MemoryChange, MemoryOrigin};
use crate::message::Message;
use crate::note::NoteChange;
use crate::plan::PlanState;
use crate::security::SecurityContext;
use crate::tool::{Tool, ToolCallState};
use crate::transcript::TranscriptEntry;
use serde::{Deserialize, Serialize};

/// The working state of one turn (issue 85): what the turn is - fixed when
/// it begins - and what it has produced and staged so far, none of it taken
/// over yet. Taking the turn over (`Turn::commit`) or giving it up
/// (`Turn::give_up`, issue 145) ends it as a whole; a branch switch (issue
/// 08) will give it up the same way.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// The envelope the turn answers, continued in the agent's name - its
    /// sender is the agent: carries the ids (`span_id`, `correlation_id`)
    /// that tie the turn to what started it.
    pub event_context: EventContext,
    /// Whose authority the turn acts with.
    pub security_context: SecurityContext,
    /// The channel's classification as it was when the turn began (issues
    /// 61, 133): the turn is consistent in itself should the channel be
    /// reclassified (issue 67).
    pub classification: Classification,
    /// The join of the classifications of everything the turn has seen so
    /// far (issue 168) - its transcript, its prompt parts, the knowledge it
    /// read, its tool results - never above `classification`: what it
    /// writes is classified so (`label`). `None` for a turn stored before
    /// it was kept, which counts as having seen its channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<Classification>,
    /// The agent's tools as they were when the turn began (issue 137): what
    /// its model is offered for every call of the turn, and what its calls
    /// are labelled and put up for approval by - one set for the whole turn,
    /// a resumed one included.
    pub tools: Vec<Tool>,
    /// What the next message of the turn is produced from, as references
    /// (issues 47, 49, 136, 137): the transcript so far - the agent's,
    /// continued by the turn's own messages - the prompt parts placed in it,
    /// the tools offered, the knowledge read, the rules of the model. Resolved by
    /// `crate::context::ContextResolver`; what is recorded with each
    /// message.
    pub context: ContextRef,
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
        let ActorId::Agent(agent) = self.event_context.sender_id else { return Vec::new() };
        self.tool_calls.iter()
            .filter(|c| c.status == ApprovalStatus::Pending)
            .map(|c| ApprovalRequest {
                id: c.id,
                session_id: self.event_context.session_id,
                channel_id: self.event_context.channel_id,
                agent_id: agent,
                tool_call: c.tool_call.clone(),
                description: c.description.clone(),
            })
            .collect()
    }

    /// Appends `message`, produced by the turn from its context as it
    /// stands, and moves the context on past it (issue 136). The message
    /// gets the digest it will have in the transcript, computed as the
    /// transcript's chain would: the turn's messages continue the agent's
    /// transcript, which does not change while a turn runs.
    pub fn push(&mut self, message: Message) {
        let context = self.context.clone();
        let digest = crate::digest::message_chain(context.transcript.as_ref(), &message);
        self.context.transcript = Some(digest);
        self.context.turn_entries += 1;
        self.entries.push(TurnEntry { node: Node::new(digest, context.transcript, TranscriptEntry::Message(message)), context });
    }

    /// The messages the turn produced so far.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter().filter_map(|e| e.node.entry.message())
    }

    /// Records that the turn read `reference` (issue 17): it counts for the
    /// messages produced from now on, and the turn has seen what it is
    /// classified (`see`). Recorded once.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        self.see_classified(reference.classification.as_ref());
        if !self.context.knowledge.contains(&reference) {
            self.context.knowledge.push(reference);
        }
    }

    /// The classification of what the turn writes - its messages, its
    /// notes, what it remembers (issue 168): what it has seen so far, at
    /// most its channel's classification when it began.
    pub fn label(&self) -> Classification {
        self.seen.clone().unwrap_or_else(|| self.classification.clone())
    }

    /// Records that the turn has seen what is classified `classification`:
    /// its label rises to the join of both (`Classification::join`). Where
    /// the join is not known to flow into the channel - no compartment in
    /// common, or only by which compartments read which - the label is the
    /// channel's classification, which everything the turn sees flows into.
    pub fn see(&mut self, classification: &Classification) {
        let channel = &self.classification;
        let label = self.label().join(classification).filter(|joined| joined.surely_flows_to(channel));
        self.seen = Some(label.unwrap_or_else(|| channel.clone()));
    }

    /// Records that the turn has seen what came with `classification`, if
    /// it came with one (`see`): what is not classified counts as the
    /// channel.
    pub fn see_classified(&mut self, classification: Option<&Classification>) {
        let channel = self.classification.clone();
        self.see(classification.unwrap_or(&channel));
    }

    /// Records that the turn has seen `message`: its classification and
    /// that of every tool result in it.
    pub fn see_message(&mut self, message: &Message) {
        self.see(&message.classification);
        for result in &message.tool_results {
            self.see(&result.classification);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::{Compartment, Confidentiality};

    #[test]
    fn a_turn_writes_as_what_it_has_seen_never_above_its_channel() {
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let channel = classified(Confidentiality::Confidential, &["hr"]);
        let mut turn = TurnState { classification: channel.clone(), seen: Some(Classification::PUBLIC), ..TurnState::default() };
        assert_eq!(turn.label(), Classification::PUBLIC, "nothing seen");

        turn.see(&Classification::general(Confidentiality::Internal));
        assert_eq!(turn.label(), Classification::general(Confidentiality::Internal));
        turn.see(&classified(Confidentiality::Internal, &["hr", "sales"]));
        assert_eq!(turn.label(), classified(Confidentiality::Internal, &["hr", "sales"]), "a wider audience than the channel's");
        turn.see(&classified(Confidentiality::Internal, &["sales"]));
        assert_eq!(turn.label(), channel, "not known to flow into the channel: the channel");
        turn.see(&Classification::PUBLIC);
        assert_eq!(turn.label(), channel, "it never falls");

        let mut unknown = TurnState { classification: channel.clone(), seen: Some(Classification::PUBLIC), ..TurnState::default() };
        unknown.see_classified(None);
        assert_eq!(unknown.label(), channel, "what is not known counts as the channel");
        let stored_before = TurnState { classification: channel.clone(), ..TurnState::default() };
        assert_eq!(stored_before.label(), channel, "a turn stored before it was kept has seen its channel");
    }
}
