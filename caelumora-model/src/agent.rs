use crate::actor::{ActorId, ActorState, EventContext};
use crate::approval::{ApprovalRequest, ApprovalStatus};
use crate::component::ComponentConfig;
use crate::digest::Digest;
use crate::external_agent::RemoteState;
use crate::confidentiality::Classification;
use crate::context::{ContextRef, KnowledgeRef};
use crate::chain::{Node, Stored};
use crate::kept::Kept;
use crate::ledger::LedgerEntry;
use crate::message::{Message, MessageId};
use crate::memory::{Memories, MemoryChain, MemoryChange, MemoryOrigin};
use crate::message_history::{HistoryEntry, MessageHistory};
use crate::transcript::{Transcript, TranscriptEntry};
use crate::note::{NoteChain, NoteChange, Notes};
use crate::name::{Name, ScopedName};
use crate::plan::PlanState;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::store::Journaled;
use crate::tool::{Tool, ToolCallState, ToolDefinition};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    NameChanged(Name),
}


define_id!(PersonalityId);

/// What kind of agent a personality makes (issue 179), and so which actor
/// runs it in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalityKind {
    /// An agent of ours: its model, tools and turns are run here, by an
    /// `AgentActor`.
    #[default]
    Internal,
    /// An agent that runs elsewhere, reached through the one component of
    /// the personality that provides `ExternalAgent`, by an
    /// `ExternalAgentActor`. Whatever else the personality wires in is not
    /// read.
    External,
}

/// An agent's traits: its components and how confidential its reasoning is.
/// Written by hand, too (a seed file, issue 105): the fields with a default
/// may be left out there (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    /// What it is found by: its name and where it lies (issue 177), the
    /// global scope unless named.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What kind of agent it makes: `internal` unless named.
    #[serde(default)]
    pub kind: PersonalityKind,
    /// The roles its agents are limited to (issue 179); see
    /// [`Personality::limiting_roles`]. Not their roles: an agent is
    /// cleared by its owner's, these can only narrow them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// What the personality is, for whoever chooses one: a catalogue line.
    #[serde(default)]
    pub description: String,
    /// How its agents are introduced (issue 176) - to the others in their
    /// channel and to themselves: who they are, what they are there for.
    /// Public, like an agent's name. Not `description`, which says what the
    /// personality is, to whoever chooses one.
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

impl Personality {
    /// The roles that limit what its agents are cleared for, besides their
    /// owner's (issue 179): an agent joins a channel only if its owner is
    /// cleared for it and, where this is `Some`, these roles are too. An
    /// internal personality that names none limits nothing; an external
    /// one that names none is limited to no roles - what is `public` - since
    /// what its agents are given leaves the system. Like every clearance,
    /// it counts only where roles are configured.
    pub fn limiting_roles(&self) -> Option<&[String]> {
        match (&self.roles, self.kind) {
            (Some(roles), _) => Some(roles),
            (None, PersonalityKind::External) => Some(&[]),
            (None, PersonalityKind::Internal) => None,
        }
    }
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
    /// What it is found by (issue 177): its name, as every channel it takes
    /// part in knows it, and where it lies. A template finds an existing
    /// agent there, so the scope decides whether the agent is reused - one
    /// per user, with its memory - or made anew for every session.
    #[serde(flatten)]
    pub name: ScopedName,
    pub personality_id: PersonalityId,
    pub security_attributes: SecurityAttributes,
}

impl Agent {
    /// The context the agent acts in, in the channel `channel` of session
    /// `session` (issue 87): on behalf of its owner, continued in its own
    /// name - whose authority it acts with is not stored with it.
    pub fn security_context(&self, session: crate::session::SessionId, channel: crate::session::ChannelId) -> SecurityContext {
        SecurityContext::new(self.security_attributes.owner_id, session, channel).next(ActorId::Agent(self.id))
    }

    /// A new agent of `personality`, owned by `owner` - whoever creates it.
    /// Whose authority it acts with is not stored with it: its actor derives
    /// that from the owner, in the channel it runs in (issue 87). Nothing of
    /// the personality is copied into it: what the agent is - its
    /// components, its introduction - is read through `personality_id`, so
    /// that a change of the personality reaches it (issue 176).
    pub fn new(name: ScopedName, personality: &Personality, owner: ActorId) -> Self {
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
            name,
            security_attributes,
        }
    }
}

/// An agent in one channel, of either kind (issue 206): what it heard and
/// said, what it was given, and the part only its kind keeps - the turn
/// under way of an agent run here, what an external agent keeps of the
/// remote side. What an external agent cannot have - notes, prompt parts,
/// tool sets, a turn - stays empty.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentChannelState {
    /// Where the agent's actor stood when the state was stored - a turn
    /// state if it was cut off in the middle of one (issue 75). The actor
    /// owns the live value and writes it in when it stores. Always `Idle`
    /// for an external agent: it is resumed from its open tasks, not from
    /// where its actor stood.
    pub processing_state: ActorState,
    /// The conversation of the channel as the agent keeps it - all that was
    /// offered to it (issue 196) - each of its own messages with what it was
    /// produced from (issue 06).
    pub history: MessageHistory,
    /// What the agent's model is given of the conversation: its own
    /// messages in full, internal steps included, and what others said
    /// (issue 50). Of an external agent, what the remote agent was given,
    /// in the order it was given (issue 179).
    pub transcript: Transcript,
    /// The notes the agent published in this channel (issue 17).
    pub notes: NoteChain,
    /// The agent's private notes in this channel, which only it reads - and
    /// only where the channel's confidentiality allows (issue 51).
    pub private_notes: NoteChain,
    /// Every prompt part the agent was given in this channel - the system
    /// prompt, notes, ... - by digest, each kept once: what the `ContextRef`s
    /// in the history point to (issues 06, 47).
    pub prompts: Kept<String>,
    /// Every set of tools the agent's model was offered in this channel, by
    /// digest, each kept once: what the `ContextRef`s in the history point
    /// to (issue 137).
    pub tool_sets: Kept<Vec<ToolDefinition>>,
    /// The turn under way, `None` between turns (issue 85). Stored with the
    /// rest, so that a turn cut off by a crash is resumed (issue 75).
    pub turn: Option<TurnState>,
    /// What an external agent keeps of the remote side (issue 179), empty
    /// for an agent run here.
    #[serde(default, skip_serializing_if = "RemoteState::is_empty")]
    pub remote: RemoteState,
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

impl AgentChannelState {
    /// Takes in what others said (issues 196, 206), the same for both kinds
    /// of agent: all that was offered to it, `heard`, into its history; what
    /// it perceived of that - what passed its perceive processors - into its
    /// transcript, which a turn under way has then seen (issue 168).
    pub fn perceive(&mut self, heard: Vec<Message>, perceived: Vec<Message>) {
        self.history.extend(heard);
        if let Some(turn) = self.turn.as_mut() {
            perceived.iter().for_each(|m| turn.see_message(m));
        }
        self.transcript.extend(perceived);
    }

    /// What the history recorded the message `id` - on whichever branch -
    /// was produced from, or `None` if this agent did not produce it (or it
    /// was before references were recorded).
    pub fn recorded_context_of(&self, id: MessageId) -> Option<ContextRef> {
        self.history.all_entries().find_map(|e| e.entry.context_of(id)).cloned()
    }
}

/// A change to an [`AgentChannelState`] since it was last stored (issue
/// 193): a variant per part of the state. A change of the agent's *state* -
/// what it heard, said, noted - as against an update of the agent itself.
/// The growing parts record what was added, the small ones are recorded
/// whole.
// A record is made once per store; its size in memory does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChannelStateChange {
    History(Stored<HistoryEntry>),
    Transcript(Stored<TranscriptEntry>),
    Notes(Stored<LedgerEntry<Notes>>),
    PrivateNotes(Stored<LedgerEntry<Notes>>),
    Prompt { digest: Digest, text: String },
    ToolSet { digest: Digest, tools: Vec<ToolDefinition> },
    ProcessingState(ActorState),
    /// The turn under way, whole: it is changed in place at every step
    /// (`prepare_resume`, the turn's own methods), and bounded by one turn.
    Turn(Option<TurnState>),
    /// What an external agent keeps of the remote side, whole: small, and
    /// bounded by its open tasks.
    Remote(RemoteState),
}

impl Journaled for AgentChannelState {
    type Change = AgentChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        let mut changes: Vec<Self::Change> = Vec::new();
        changes.extend(self.history.changes().map(AgentChannelStateChange::History));
        changes.extend(self.transcript.changes().map(AgentChannelStateChange::Transcript));
        changes.extend(self.notes.changes().map(AgentChannelStateChange::Notes));
        changes.extend(self.private_notes.changes().map(AgentChannelStateChange::PrivateNotes));
        changes.extend(self.prompts.changes().into_iter().map(|(digest, text)| AgentChannelStateChange::Prompt { digest, text }));
        changes.extend(self.tool_sets.changes().into_iter().map(|(digest, tools)| AgentChannelStateChange::ToolSet { digest, tools }));
        changes.push(AgentChannelStateChange::ProcessingState(self.processing_state));
        changes.push(AgentChannelStateChange::Turn(self.turn.clone()));
        changes.push(AgentChannelStateChange::Remote(self.remote.clone()));
        changes
    }

    fn mark_stored(&mut self) {
        self.history.mark_stored();
        self.transcript.mark_stored();
        self.notes.mark_stored();
        self.private_notes.mark_stored();
        self.prompts.mark_stored();
        self.tool_sets.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            AgentChannelStateChange::History(change) => self.history.apply(change)?,
            AgentChannelStateChange::Transcript(change) => self.transcript.apply(change)?,
            AgentChannelStateChange::Notes(change) => self.notes.apply(change)?,
            AgentChannelStateChange::PrivateNotes(change) => self.private_notes.apply(change)?,
            AgentChannelStateChange::Prompt { digest, text } => self.prompts.apply(digest, text),
            AgentChannelStateChange::ToolSet { digest, tools } => self.tool_sets.apply(digest, tools),
            AgentChannelStateChange::ProcessingState(state) => self.processing_state = state,
            AgentChannelStateChange::Turn(turn) => self.turn = turn,
            AgentChannelStateChange::Remote(remote) => self.remote = remote,
        }
        Ok(())
    }
}

/// The state of an agent itself, across its channels and - for agents of
/// user or global scope - across sessions (issue 62). One owner per agent
/// and process holds it; turns read a snapshot and stage their changes
/// (`TurnState::memory`), which are applied to the current state when the
/// turn completes, so that no turn overwrites another's changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    /// What the agent remembers across its channels and sessions.
    pub memory: MemoryChain,
}

/// A change to an [`AgentState`] since it was last stored (issue 193).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStateChange {
    Memory(Stored<LedgerEntry<Memories>>),
}

impl Journaled for AgentState {
    type Change = AgentStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        self.memory.changes().map(AgentStateChange::Memory).into_iter().collect()
    }

    fn mark_stored(&mut self) {
        self.memory.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            AgentStateChange::Memory(change) => self.memory.apply(change),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::actor::ActorState;
    use crate::confidentiality::{Compartment, Confidentiality};

    #[test]
    fn a_personality_is_internal_unless_named_external() {
        let id = PersonalityId::new();
        let written = |kind: &str| serde_json::json!({ "id": id, "name": "plain", "kind": kind });
        let plain: Personality = serde_json::from_value(serde_json::json!({ "id": id, "name": "plain" })).unwrap();
        assert_eq!(plain.kind, PersonalityKind::Internal, "left out");
        let external: Personality = serde_json::from_value(written("external")).unwrap();
        assert_eq!(external.kind, PersonalityKind::External);
        assert!(serde_json::from_value::<Personality>(written("remote")).is_err(), "no other kind");
    }

    #[test]
    fn a_personality_limits_its_agents_only_where_it_names_roles_unless_external() {
        let mut personality: Personality = serde_json::from_value(serde_json::json!({ "id": PersonalityId::new(), "name": "plain" })).unwrap();
        assert_eq!(personality.limiting_roles(), None, "internal, none named: the owner's alone");
        personality.kind = PersonalityKind::External;
        assert_eq!(personality.limiting_roles(), Some(&[][..]), "external, none named: public only");
        personality.roles = Some(vec!["staff".to_string()]);
        assert_eq!(personality.limiting_roles(), Some(&["staff".to_string()][..]));
        personality.kind = PersonalityKind::Internal;
        assert_eq!(personality.limiting_roles(), Some(&["staff".to_string()][..]), "named, internal too");
    }

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

    /// `state` as a store holds it, and the live state marked stored.
    pub(crate) fn stored<T: Journaled>(state: &mut T) -> T {
        let copy = serde_json::from_value(serde_json::to_value(&*state).unwrap()).unwrap();
        state.mark_stored();
        copy
    }

    /// Replays the changes of `state` onto `copy` - twice, since replaying
    /// is idempotent - and asserts that both are the same afterwards.
    pub(crate) fn replayed<T: Journaled>(state: &T, copy: &mut T) {
        for _ in 0..2 {
            for change in state.changes() {
                copy.apply(change).unwrap();
            }
        }
        assert_eq!(serde_json::to_value(&*copy).unwrap(), serde_json::to_value(state).unwrap());
        // The small parts are told whole every time; the growing ones must
        // have recorded nothing: as a copy loaded afresh.
        let reloaded: T = serde_json::from_value(serde_json::to_value(&*copy).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(copy.changes()).unwrap(), serde_json::to_value(reloaded.changes()).unwrap(), "replaying records nothing");
    }

    #[test]
    fn what_changed_in_an_agents_channel_state_replays_onto_what_was_stored() {
        use crate::message::tests::{id, make_message};
        use crate::note::NoteId;
        let mut state = AgentChannelState::default();
        state.history.extend(vec![make_message(id(1))]);
        state.transcript.push(make_message(id(1)));
        let mut copy = stored(&mut state);

        let before = state.transcript.push(make_message(id(2)));
        state.history.push(make_message(id(2)), Some(ContextRef { transcript: before, ..ContextRef::default() }));
        state.notes.push(NoteChange::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, Default::default(), id(2));
        state.private_notes.push(NoteChange::Add { id: NoteId(1), text: "mine".to_string(), classification: Classification::CHANNEL_DEFAULT }, Default::default(), id(2));
        state.prompts.keep(crate::digest::prompt("system"), || "system".to_string());
        state.tool_sets.keep(crate::digest::tools(&[]), Vec::new);
        state.processing_state = ActorState::WaitingForApproval;
        state.turn = Some(TurnState { model_calls: 3, ..TurnState::default() });

        let changes = state.changes();
        let parts: Vec<&str> = changes.iter().map(|c| match c {
            AgentChannelStateChange::History(_) => "history",
            AgentChannelStateChange::Transcript(_) => "transcript",
            AgentChannelStateChange::Notes(_) => "notes",
            AgentChannelStateChange::PrivateNotes(_) => "private_notes",
            AgentChannelStateChange::Prompt { .. } => "prompt",
            AgentChannelStateChange::ToolSet { .. } => "tool_set",
            AgentChannelStateChange::ProcessingState(_) => "processing_state",
            AgentChannelStateChange::Turn(_) => "turn",
            AgentChannelStateChange::Remote(_) => "remote",
        }).collect();
        assert_eq!(parts, ["history", "transcript", "notes", "private_notes", "prompt", "tool_set", "processing_state", "turn", "remote"], "what was added, then the small parts whole");
        replayed(&state, &mut copy);

        state.mark_stored();
        state.turn = None;
        assert_eq!(state.changes().len(), 3, "between turns a step is the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.turn.is_none());
    }

    #[test]
    fn all_that_is_heard_is_kept_and_what_is_perceived_is_seen_by_a_turn_under_way() {
        use crate::confidentiality::Confidentiality;
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.perceive(vec![make_message(id(1)), make_message(id(2))], vec![make_message(id(1))]);
        assert_eq!(state.history.messages().len(), 2, "all that was heard");
        assert_eq!(state.transcript.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1)], "what was perceived");

        let channel = Classification::general(Confidentiality::Confidential);
        state.turn = Some(TurnState { classification: channel.clone(), seen: Some(Classification::PUBLIC), ..TurnState::default() });
        let confidential = Message { classification: channel.clone(), ..make_message(id(3)) };
        state.perceive(vec![confidential.clone()], vec![confidential]);
        assert_eq!(state.turn.as_ref().map(TurnState::label), Some(channel), "the turn has seen it");
    }

    #[test]
    fn what_changed_in_an_external_agents_state_replays_onto_what_was_stored() {
        use crate::external_agent::{ConversationId, OpenTask, OpenTaskState, TaskId};
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.history.extend(vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(copy.remote.is_empty(), "nothing kept of the remote side yet");

        state.history.extend(vec![make_message(id(2))]);
        state.transcript.extend(vec![make_message(id(1)), make_message(id(2))]);
        let remote = &mut state.remote;
        remote.sent_up_to = state.transcript.digest();
        remote.conversation = Some(ConversationId("c-1".to_string()));
        remote.open_tasks.push(OpenTask { task: TaskId("t-1".to_string()), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: Some("so far".to_string()), requested: None });
        assert_eq!(state.changes().len(), 2 + 3, "the history's, the transcript's, the small parts");
        replayed(&state, &mut copy);

        state.mark_stored();
        state.remote.open_tasks.clear();
        assert_eq!(state.changes().len(), 3, "the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.remote.open_tasks.is_empty());
    }

    #[test]
    fn what_changed_in_an_agents_own_state_replays_onto_what_was_stored() {
        use crate::memory::{MemoryChange, MemoryId};
        let mut state = AgentState::default();
        state.memory.push(MemoryChange::Add { id: MemoryId(1), text: "a".to_string(), classification: Classification::CHANNEL_DEFAULT }, Default::default(), MessageId::default());
        let mut copy = stored(&mut state);
        assert!(state.changes().is_empty(), "nothing between turns");
        state.memory.push(MemoryChange::Remove { id: MemoryId(1) }, Default::default(), MessageId::default());
        assert!(matches!(state.changes().as_slice(), [AgentStateChange::Memory(_)]));
        replayed(&state, &mut copy);
        assert!(copy.memory.current().is_empty());
    }
}
