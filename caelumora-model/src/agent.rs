use crate::actor::{ActorId, ActorState, EventContext};
use crate::component::ComponentConfig;
use crate::digest::Digest;
use crate::external_agent::RemoteState;
use crate::confidentiality::Classification;
use crate::context::{ContextRef, ModelProfile, PromptRef};
use crate::chain::Stored;
use crate::kept::Kept;
use crate::ledger::LedgerEntry;
use crate::message::{Message, MessageId, MessageRole};
use crate::memory::{Memories, MemoryChain};
use crate::message_history::{HistoryEntry, MessageHistory};
use crate::transcript::{GivenUp, GivenUpReason, Transcript, TranscriptEntry};
use crate::turn::{TurnEntry, TurnState};
use crate::work::Work;
use crate::note::{NoteChain, NoteChange, NoteOrigin, Notes};
use crate::name::{Name, ScopedName};
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::store::Journaled;
use crate::tool::{Tool, ToolDefinition};
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

impl AgentChannelState {
    /// Takes in what others said (issues 196, 206), the same for both kinds
    /// of agent: all that was offered to it, `heard`, into its history; what
    /// it perceived of that - what passed its perceive processors - into its
    /// transcript, which a turn under way has then seen (issue 168).
    pub fn perceive(&mut self, heard: Vec<Message>, perceived: Vec<Message>) {
        self.history.extend(heard);
        if let Some(turn) = self.turn.as_mut() {
            perceived.iter().for_each(|m| turn.work.see_message(m));
        }
        self.transcript.extend(perceived);
    }

    /// Keeps the text of a prompt part the agent was given once by digest
    /// (issue 47), and returns the reference to it: given in `role` after
    /// the entry `after` - `None`: before everything - its text classified
    /// `classification`.
    pub fn keep_prompt(&mut self, role: MessageRole, text: String, after: Option<Digest>, classification: Option<Classification>) -> PromptRef {
        let digest = crate::digest::prompt(&text);
        self.prompts.keep(digest, || text);
        PromptRef { text: digest, role, after, classification }
    }

    /// Begins a turn (issue 85): fixes what it is - the envelope it answers,
    /// whose authority it acts with, the channel's classification now, how
    /// the agent's model is given its context (the `profile`, issues 49, 63),
    /// and the agent's `tools` (issue 137), whose set is kept once by digest.
    /// All but the first two hold for the whole turn - also one resumed after
    /// the model's configuration or the agent's tools changed - so that what
    /// it reads, publishes and remembers is judged alike, and what its model
    /// is given and offered too. The turn has seen the transcript, which is
    /// what the model is given first (issue 168). Whatever a previous turn
    /// left behind is gone.
    pub fn begin_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: Classification,
        tools: Vec<Tool>,
        profile: ModelProfile,
    ) {
        let offered = (!tools.is_empty()).then(|| {
            let definitions: Vec<ToolDefinition> = tools.iter().map(Tool::definition).collect();
            let digest = crate::digest::tools(&definitions);
            self.tool_sets.keep(digest, || definitions);
            digest
        });
        let mut work = Work::new(event_context, security_context, classification);
        work.context = ContextRef { transcript: self.transcript.digest(), tools: offered, profile, ..ContextRef::default() };
        self.transcript.entries().filter_map(|e| e.entry.message()).for_each(|m| work.see_message(m));
        let turn = TurnState { work, tools, ..TurnState::default() };
        self.turn = Some(turn);
    }

    /// Takes over what a turn produced (issues 06, 17, 136): its `entries`
    /// into the transcript - and those not internal into the history, each
    /// with what it was produced from - and its changes to the agent's
    /// `notes` and `private_notes` here, anchored at its last message in the
    /// history. Returns whether there were changes to the notes.
    pub fn record_turn(&mut self, entries: Vec<TurnEntry>, notes: Vec<NoteChange>, private_notes: Vec<NoteChange>) -> bool {
        let expected = entries.last().map(|e| e.node.digest);
        for TurnEntry { node, context } in entries {
            let TranscriptEntry::Message(message) = node.entry else { continue };
            self.transcript.push(message.clone());
            if !message.internal {
                self.history.push(message, Some(context));
            }
        }
        // The turn's messages got their digests continuing the transcript
        // as it was when the turn began (issue 136); had it changed
        // meanwhile, what they were produced from could not be told.
        if expected.is_some() && self.transcript.digest() != expected {
            tracing::warn!("An agent's transcript changed during a turn; the contexts recorded for the turn's messages cannot be resolved");
        }

        // A message of the history, not an internal step of the transcript:
        // what branching (issue 08) will find the changes by.
        let after = self.history.last_message_id();
        let origin = NoteOrigin { transcript: self.transcript.digest() };
        let changed = !notes.is_empty() || !private_notes.is_empty();
        for change in notes {
            self.notes.push(change, origin.clone(), after);
        }
        for change in private_notes {
            self.private_notes.push(change, origin.clone(), after);
        }
        changed
    }

    /// Keeps what a turn given up for `reason` produced, its `entries`, as a
    /// branch that is no longer active (issue 145): recorded as when taken
    /// over (`record_turn`), each with what it was produced from, then both
    /// heads moved back to where the turn began, and the transcript records
    /// the turn as given up, naming the branch. A turn that produced
    /// nothing leaves nothing.
    pub fn give_up_turn(&mut self, entries: Vec<TurnEntry>, reason: GivenUpReason) {
        let turn_entries = entries.len();
        if turn_entries == 0 {
            return;
        }
        let transcript = self.transcript.digest();
        let history = self.history.digest();
        self.record_turn(entries, Vec::new(), Vec::new());
        let end = self.transcript.digest();
        self.transcript.move_head(transcript.as_ref());
        self.history.move_head(history.as_ref());
        if let Some(end) = end {
            self.transcript.give_up(end, GivenUp { reason, at: chrono::Utc::now(), turn_entries });
        }
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
    use crate::confidentiality::Confidentiality;
    use crate::tool::{ToolParameters, ToolVisibility};

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

    fn said(content: &str, classification: Classification) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::new(classification) }
    }

    fn a_state_that_heard(content: &str, classification: Classification) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let message = said(content, classification);
        state.perceive(vec![message.clone()], vec![message]);
        state
    }

    fn begin(state: &mut AgentChannelState, tools: Vec<Tool>) {
        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT, tools, ModelProfile::default());
    }

    #[test]
    fn a_turn_begun_has_seen_the_transcript_and_offers_its_tools_kept_once() {
        let internal = Classification::general(Confidentiality::Internal);
        let mut state = a_state_that_heard("hello", internal.clone());
        let tool = Tool {
            name: "calc".to_string(),
            description: "calculates".to_string(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
        };
        begin(&mut state, vec![tool.clone()]);
        begin(&mut state, vec![tool]);
        let turn = state.turn.as_ref().expect("a turn was begun");
        assert_eq!(turn.work.label(), internal, "it has seen the transcript");
        assert_eq!(turn.work.context.transcript, state.transcript.digest(), "it continues the transcript");
        assert!(turn.work.context.tools.is_some() && state.tool_sets.len() == 1, "the set offered is kept once");

        begin(&mut state, Vec::new());
        assert!(state.turn.as_ref().is_some_and(|t| t.work.context.tools.is_none()), "no tools, no set");
    }

    #[test]
    fn a_turn_recorded_continues_the_transcript_and_keeps_in_the_history_what_is_not_internal() {
        let mut state = a_state_that_heard("add 1 and 2", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        let mut turn = state.turn.take().expect("a turn was begun");
        turn.push(Message { internal: true, ..said("thinking", Classification::CHANNEL_DEFAULT) });
        let answer = said("3", Classification::CHANNEL_DEFAULT);
        turn.push(answer.clone());
        let produced_from = turn.entries[1].context.clone();
        let expected = turn.entries.last().map(|e| e.node.digest);
        let note = NoteChange::Add { id: crate::note::NoteId(1), text: "sums".to_string(), classification: Classification::CHANNEL_DEFAULT };

        assert!(state.record_turn(turn.entries, vec![note], Vec::new()), "a note changed");
        assert_eq!(state.transcript.digest(), expected, "the turn's messages have the digests they were given");
        assert_eq!(state.transcript.entries().count(), 3);
        assert_eq!(state.history.messages().iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>(), ["add 1 and 2", "3"]);
        assert_eq!(state.recorded_context_of(answer.id), Some(produced_from), "with what it was produced from");
        assert_eq!(state.notes.current().len(), 1);
    }

    #[test]
    fn a_turn_given_up_is_a_branch_no_longer_active_and_names_it() {
        let mut state = a_state_that_heard("mail bob", Classification::CHANNEL_DEFAULT);
        let history = state.history.digest();
        begin(&mut state, Vec::new());
        let mut turn = state.turn.take().expect("a turn was begun");
        turn.push(said("mailing", Classification::CHANNEL_DEFAULT));

        state.give_up_turn(turn.entries, GivenUpReason::Stopped);
        assert_eq!(state.history.digest(), history, "the history is back where it began");
        let active: Vec<String> = state.transcript.entries().filter_map(|e| e.entry.message()).map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(active, ["mail bob"], "and so is the transcript, followed by the record");
        let given_up = state.transcript.given_up_turns();
        assert_eq!(given_up.len(), 1);
        assert_eq!(given_up[0].given_up.turn_entries, 1);
        assert_eq!(given_up[0].messages.iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>(), ["mailing"]);

        state.give_up_turn(Vec::new(), GivenUpReason::Stopped);
        assert_eq!(state.transcript.given_up_turns().len(), 1, "a turn that produced nothing leaves nothing");
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
        state.turn = Some(TurnState { work: Work::new(Default::default(), Default::default(), channel.clone()), ..TurnState::default() });
        let confidential = Message { classification: channel.clone(), ..make_message(id(3)) };
        state.perceive(vec![confidential.clone()], vec![confidential]);
        assert_eq!(state.turn.as_ref().map(|t| t.work.label()), Some(channel), "the turn has seen it");
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
        remote.conversation = Some(ConversationId("c-1".to_string()));
        remote.open_tasks.push(OpenTask { task: TaskId("t-1".to_string()), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: Some("so far".to_string()), work: None });
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
