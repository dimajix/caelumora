use crate::actor::{ActorId, ActorState, EventContext};
use crate::approval::{ApprovalRequest, ApprovalStatus};
use crate::component::ComponentConfig;
use crate::digest::Digest;
use crate::confidentiality::Classification;
use crate::context::{ContextRef, KnowledgeRef};
use crate::message::{Message, MessageId};
use crate::message_history::MessageHistory;
use crate::transcript::{Transcript, TranscriptEntry};
use crate::note::{NoteChain, NoteChange, NoteOrigin};
use crate::meta::{Metadata, Scope};
use crate::plan::PlanState;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::tool::{Tool, ToolCallState, ToolDefinition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    NameChanged(String),
}


define_id!(PersonalityId);

/// An agent's traits: its components and how confidential its reasoning is.
/// Written by hand, too (a seed file, issue 105): the fields with a default
/// may be left out there (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    pub metadata: Metadata,
    /// How its agents are introduced (issue 176) - to the others in their
    /// channel and to themselves: who they are, what they are there for.
    /// Public, like an agent's name. Not `metadata.description`, which says
    /// what the personality is, to whoever chooses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    /// Whether the agent first analyzes the request and proposes a
    /// structured plan requiring user approval before executing it, instead
    /// of answering directly, is derived from whether `components` wires in
    /// the `create_plan_tool` component - not a separate flag here, since
    /// extended planning is just another tool a personality opts into.
    #[serde(default)]
    pub components: ComponentConfig,
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
    /// Who sees the reasoning of its agents (issues 61, 133): `owner` keeps
    /// it from the channel always; otherwise what a turn saw decides.
    /// `channel` unless named.
    #[serde(default)]
    pub reasoning_visibility: crate::confidentiality::Visibility,
}

impl Secured for Personality {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Secured for Agent {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}


define_id!(AgentId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub metadata: Metadata,
    pub personality_id: PersonalityId,
    pub security_attributes: SecurityAttributes,
}

impl Agent {
    /// A new agent of `personality`, owned by `owner` - whoever creates it.
    /// Whose authority it acts with is not stored with it: its actor derives
    /// that from the owner, in the channel it runs in (issue 87). Nothing of
    /// the personality is copied into it: what the agent is - its
    /// components, its introduction - is read through `personality_id`, so
    /// that a change of the personality reaches it (issue 176).
    pub fn new(name:String, scope:Scope, personality: &Personality, owner: ActorId) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        // The agent itself contributes, so that it may read its own
        // definition (issue 11).
        let security_attributes = SecurityAttributes {
            owner_id: owner,
            contributors: vec![ActorId::Agent(agent_id)],
        };
        Agent {
            id: agent_id,
            personality_id: personality.id,
            metadata: Metadata {
                name,
                scope,
                ..Metadata::default()
            },
            security_attributes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentChannelState {
    /// Where the agent's actor stood when the state was stored - a turn
    /// state if it was cut off in the middle of one (issue 75). The actor
    /// owns the live value and writes it in when it stores.
    pub processing_state: ActorState,
    /// The conversation of the channel as the agent keeps it, each of its
    /// own messages with what it was produced from (issue 06).
    pub history: MessageHistory,
    /// What the agent's model is given of the conversation: its own
    /// messages in full, internal steps included, and what others said
    /// (issue 50).
    pub transcript: Transcript,
    /// The notes the agent published in this channel (issue 17).
    pub notes: NoteChain,
    /// The agent's private notes in this channel, which only it reads - and
    /// only where the channel's confidentiality allows (issue 51).
    pub private_notes: NoteChain,
    /// Every prompt part the agent was given in this channel - the system
    /// prompt, notes, ... - by digest, each kept once: what the `ContextRef`s
    /// in the history point to (issues 06, 47).
    pub prompts: BTreeMap<Digest, String>,
    /// Every set of tools the agent's model was offered in this channel, by
    /// digest, each kept once: what the `ContextRef`s in the history point
    /// to (issue 137).
    pub tool_sets: BTreeMap<Digest, Vec<ToolDefinition>>,
    /// The turn under way, `None` between turns (issue 85). Stored with the
    /// rest, so that a turn cut off by a crash is resumed (issue 75).
    pub turn: Option<TurnState>,
}

/// The working state of one turn (issue 85): what the turn is - fixed when
/// it begins - and what it has produced and staged so far, none of it taken
/// over yet. Taking the turn over (`commit_turn`) or giving it up
/// (`give_up_turn`, issue 145) ends it as a whole; a branch switch (issue
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
    /// Changes to the agent's `AgentState`, applied to the current state
    /// when the turn completes (issue 62).
    pub agent_changes: Vec<AgentStateChange>,
    /// Changes to the agent's state in this channel - to its notes and
    /// private notes - applied when the turn completes (issue 65).
    pub channel_changes: Vec<AgentChannelChange>,
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
        self.entries.push(TurnEntry { entry: TranscriptEntry { message, digest, given_up: None }, context });
    }

    /// The messages the turn produced so far.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter().map(|e| &e.entry.message)
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
    pub entry: TranscriptEntry,
    pub context: ContextRef,
}

/// A change to an [`AgentChannelState`], staged by a turn and applied when
/// the turn completes (issue 65) - like [`AgentStateChange`], but without a
/// handle: the state of an agent in a channel has one writer, the agent's
/// actor in that channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentChannelChange {
    /// A change to the agent's notes in the channel (issue 17).
    Note(NoteChange),
    /// A change to its private notes in the channel (issue 51).
    PrivateNote(NoteChange),
}

impl AgentChannelState {
    /// Applies `change`, made in the turn that ended with message `after`
    /// while the agent's transcript had the digest `transcript`.
    pub fn apply(&mut self, change: AgentChannelChange, after: MessageId, transcript: Option<Digest>) {
        match change {
            AgentChannelChange::Note(change) => self.notes.push(change, NoteOrigin { transcript }, after),
            AgentChannelChange::PrivateNote(change) => self.private_notes.push(change, NoteOrigin { transcript }, after),
        }
    }

    /// What the history recorded the message `id` - on whichever branch -
    /// was produced from, or `None` if this agent did not produce it (or it
    /// was before references were recorded).
    pub fn recorded_context_of(&self, id: MessageId) -> Option<ContextRef> {
        self.history.all_entries().find(|e| e.message.id == id)?.context.clone()
    }
}

/// The state of an agent itself, across its channels and - for agents of
/// user or global scope - across sessions (issue 62). One owner per agent
/// and process holds it; turns read a snapshot and stage their changes as
/// [`AgentStateChange`]s, which are applied to the current state when the
/// turn completes, so that no turn overwrites another's changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    /// What the agent remembers across its channels and sessions.
    pub memory: crate::memory::MemoryChain,
}

/// A change to an [`AgentState`], staged by a turn and applied to the state
/// as it is when the turn completes (issue 62).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentStateChange {
    /// A change to the agent's memory, made at `origin`.
    Memory { change: crate::memory::MemoryChange, origin: crate::memory::MemoryOrigin },
}

impl AgentState {
    /// Applies `change`, made in the turn that ended with message `after`.
    pub fn apply(&mut self, change: AgentStateChange, after: MessageId) {
        match change {
            AgentStateChange::Memory { change, origin } => self.memory.push(change, origin, after),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorState;
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

    #[test]
    fn a_turn_under_way_is_stored_with_the_state_and_none_between_turns() {
        let idle = AgentChannelState::default();
        let json = serde_json::to_string(&idle).unwrap();
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(decoded.turn.is_none() && decoded.processing_state == ActorState::Idle);

        let waiting = AgentChannelState {
            processing_state: ActorState::WaitingForApproval,
            turn: Some({
                let mut turn = TurnState::default();
                turn.push(Message::new(crate::confidentiality::Classification::CHANNEL_DEFAULT));
                turn
            }),
            ..AgentChannelState::default()
        };
        let json = serde_json::to_string(&waiting).unwrap();
        assert!(json.contains("\"WaitingForApproval\""));
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.processing_state, ActorState::WaitingForApproval);
        assert_eq!(decoded.turn.map(|t| t.messages().count()), Some(1), "the turn is resumed from what was stored");
    }

    #[test]
    fn a_channel_change_goes_to_the_chain_it_names() {
        use crate::note::NoteId;
        let mut state = AgentChannelState::default();
        let note = |id, text: &str| NoteChange::Add { id: NoteId(id), text: text.to_string(), classification: crate::confidentiality::Classification::CHANNEL_DEFAULT };
        state.apply(AgentChannelChange::Note(note(1, "public")), MessageId::new(), None);
        state.apply(AgentChannelChange::PrivateNote(note(1, "private")), MessageId::new(), None);
        assert_eq!(state.notes.current()[0].text, "public");
        assert_eq!(state.private_notes.current()[0].text, "private");
    }
}
