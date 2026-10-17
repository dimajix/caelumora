use crate::actor::{ActorId, ActorState, EventContext};
use crate::approval::ApprovalRequest;
use crate::chain::Stored;
use crate::component::ComponentConfig;
use crate::confidentiality::{Classification, Visibility};
use crate::context::{ContextRef, ContextResolver, KnowledgeRef, ModelProfile, PromptRef};
use crate::digest::Digest;
use crate::external_agent::RemoteState;
use crate::history::{HistoryEntry, History, WINDOW};
use crate::kept::Kept;
use crate::ledger::LedgerEntry;
use crate::memory::{Memories, MemoryChain};
use crate::message::{Message, MessageId, MessageRole, TokenUsage, executed_calls};
use crate::note::{NoteChain, NoteChange, NoteOrigin, Notes};
use crate::removal::{Deletable, Removal};
use crate::role::RoleId;
use crate::scope::{ScopeField, Scoped};
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::store::{self, EntityKind, Journaled, NoEntity, Updatable};
use crate::tool::{ToolCallResult, ToolCallStatus, ToolResultStatus, ToolSet};
use crate::transcript::{as_they_stand, Transcript, TranscriptEntry};
use crate::turn::GivenUpReason;
use crate::turn::{CommandRequest, TurnState};
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Description, Introduction};
use crate::value::ValueError;
use crate::work::{TurnId, Work};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::mem;
use tracing::warn;
use uuid::Uuid;
use chrono::Utc;
use crate::digest;
use crate::question::QuestionRequest;
use crate::session::{ChannelId, SessionId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    /// Renamed, as it was asked: the agent's name is made from it as the
    /// change is applied (issue 248).
    NameChanged(String),
}


define_id!(PersonalityId);

/// What kind of agent a personality makes (issue 179), and so which actor
/// runs it in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
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

impl PersonalityKind {
    /// Whether an agent of this kind can be asked to answer again instead
    /// of one of its messages (issue 234): not one that runs elsewhere,
    /// whose remote side keeps what it was given (`RetryRefused::Elsewhere`).
    pub fn can_retry(self) -> bool {
        match self {
            Self::Internal => true,
            Self::External => false,
        }
    }
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
    pub roles: Option<Vec<RoleId>>,
    /// What the personality is, for whoever chooses one: a catalogue line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// How its agents are introduced (issue 176) - to the others in their
    /// channel and to themselves: who they are, what they are there for.
    /// Public, like an agent's name. Not `description`, which says what the
    /// personality is, to whoever chooses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduction: Option<Introduction>,
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
    pub reasoning_visibility: Visibility,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Personality {
    /// The roles that limit what its agents are cleared for, besides their
    /// owner's (issue 179): an agent joins a channel only if its owner is
    /// cleared for it and, where this is `Some`, these roles are too. An
    /// internal personality that names none limits nothing; an external
    /// one that names none is limited to no roles - what is `public` - since
    /// what its agents are given leaves the system. Like every clearance,
    /// it counts only where roles are configured.
    pub fn limiting_roles(&self) -> Option<&[RoleId]> {
        match (&self.roles, self.kind) {
            (Some(roles), _) => Some(roles),
            (None, PersonalityKind::External) => Some(&[]),
            (None, PersonalityKind::Internal) => None,
        }
    }
}

// `store::Stored`, beside the `chain::Stored` of an agent's state.
impl store::Stored for Personality {
    const KIND: EntityKind = EntityKind::new("personality");
    type Id = PersonalityId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> PersonalityId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }
}

impl Deletable for Personality {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

impl store::Stored for Agent {
    const KIND: EntityKind = EntityKind::new("agent");
    type Id = AgentId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> AgentId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    /// An agent acts in its owner's name (issue 341): whoever may change it
    /// cannot lend it another's rights by naming them as its principal.
    fn validate(&self) -> Result<(), String> {
        if self.principal_id == self.security_attributes.owner_id {
            Ok(())
        } else {
            Err("an agent acts in its owner's name".to_string())
        }
    }
}

/// What a change of the agent says is made into its values here (issue
/// 248).
impl Updatable for Agent {
    type Update = AgentUpdateEvent;

    fn apply(&mut self, event: &AgentUpdateEvent) -> Result<(), ValueError> {
        match event {
            AgentUpdateEvent::NameChanged(name) => self.name.name = Name::new(name)?,
        }
        Ok(())
    }
}

impl Deletable for Agent {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
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

impl Scoped for Personality {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

impl Scoped for Agent {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}


define_id!(AgentId);

/// The agent's own state changed - its memory (issue 62), announced as
/// `EntityChange::Custom` (issue 323) with the agent's attributes once the
/// change is stored; without content, since the state is private: whoever
/// may see it fetches it.
pub const STATE_CHANGED: &str = "state_changed";

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
    /// In whose name the agent acts - whose rights it exercises (issue
    /// 341). Its owner when it is made, and its owner alone for now: whom
    /// else an agent may act for is issue 340.
    pub principal_id: ActorId,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Agent {
    /// The context the agent acts in when nobody asked it, in the channel
    /// `channel` of session `session` (issue 87): begun by the agent itself,
    /// answered for by its owner and acting in the name of its principal
    /// (issues 255, 341).
    pub fn security_context(&self, session: SessionId, channel: ChannelId) -> SecurityContext {
        let agent = ActorId::Agent(self.id);
        SecurityContext::new(agent, session, channel).next(agent, self.security_attributes.owner_id, self.principal_id)
    }

    /// A new agent of the personality `personality_id`, owned by `owner` -
    /// whoever creates it - and acting in their name (issue 341). Nothing of
    /// the personality is copied into it: what the agent is - its
    /// components, its introduction - is read through `personality_id`, so
    /// that a change of the personality reaches it (issue 176).
    pub fn new(name: ScopedName, personality_id: PersonalityId, owner: ActorId) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        // Read by its owner, whose rights it acts with (issue 255): it reads
        // its own definition as its owner.
        let security_attributes = SecurityAttributes::owned_by(owner);
        Agent {
            id: agent_id,
            personality_id,
            name,
            security_attributes,
            principal_id: owner,
            deleted: None,
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
    /// owns the live value and writes it in when it stores. An external
    /// agent does not resume from it: it is resumed from its open tasks,
    /// and starts `Idle` (issue 363).
    pub processing_state: ActorState,
    /// The conversation of the channel as the agent keeps it - all that was
    /// offered to it (issue 196) - each of its own messages with what it was
    /// produced from (issue 06). From `history_from` on, as a store loads it:
    /// not part of the state's document, but a chain a store keeps apart
    /// (issue 379), written with the state and read on its own
    /// (`HistoryStore`).
    #[serde(skip)]
    pub history: History,
    /// The position from which on the agent keeps its history in memory; a
    /// store loads the history from there.
    pub history_from: u64,
    /// What the agent's model is given of the conversation: its own
    /// messages in full, internal steps included, and what others said
    /// (issue 50). Of an external agent, what the remote agent was given,
    /// in the order it was given (issue 179). From `transcript_from` on, as
    /// a store loads it: not part of the state's document, but a chain a
    /// store keeps apart (issue 380), written with the state and read on its
    /// own (`TranscriptStore`).
    #[serde(skip)]
    pub transcript: Transcript,
    /// The position from which on the agent keeps its transcript in memory;
    /// a store loads the transcript from there.
    pub transcript_from: u64,
    /// The notes the agent published in this channel (issue 17).
    pub notes: NoteChain,
    /// The agent's private notes in this channel, which only it reads - and
    /// only where the channel's confidentiality allows (issue 51).
    pub private_notes: NoteChain,
    /// The prompt parts the agent was given in this channel - the system
    /// prompt, notes, ... - by digest, each kept once: what the `ContextRef`s
    /// in the history point to (issues 06, 47). Written by the store apart
    /// from the document, before everything that names them, and read by
    /// digest (`TranscriptStore::prompts`, issue 383); a state loaded holds
    /// those its turn under way names.
    #[serde(skip)]
    pub prompts: Kept<String>,
    /// The tool sets the agent's model was offered in this channel, by
    /// digest, each offer kept once: what the `ContextRef`s in the history
    /// point to (issues 137, 194). Kept as the prompt parts are.
    #[serde(skip)]
    pub offered_tools: Kept<Vec<ToolSet>>,
    /// The turn under way, `None` between turns (issue 85). Stored with the
    /// rest, so that a turn cut off by a crash is resumed (issue 75).
    pub turn: Option<TurnState>,
    /// What the agent's model calls in this channel used (issue 209), each
    /// counted once, as the message it produced is recorded - said, or
    /// replacing older messages in a compaction (`TokenUsage::counted`) -
    /// rather than summed over the transcript (issue 376). `None` while no
    /// call reported its tokens; an external agent's never do. How full the
    /// context is, is the last call's measure, and after a compaction the
    /// new branch as estimated then (`compact`, issue 390): never read anew,
    /// however often it is asked for.
    pub used: Option<TokenUsage>,
    /// What an external agent keeps of the remote side (issue 179), empty
    /// for an agent run here.
    #[serde(default, skip_serializing_if = "RemoteState::is_empty")]
    pub remote: RemoteState,
}

impl AgentChannelState {
    /// Lets go of what its history and its transcript hold before their
    /// windows once they are stored, and records where the windows begin,
    /// to be stored with the next put (issues 379, 380).
    ///
    /// The history from the first message of the turn under way on - what
    /// the turn said is its answer (`History::said_in_turn`) - and at least
    /// the last `WINDOW` entries, whatever the agent compacted: the history
    /// is the record of what was said, and nothing that builds what the
    /// model is given reads it (issue 387); what reads it in memory reads
    /// the turn under way and its end. The transcript from the root
    /// of its active branch on: every reader in memory reads that branch
    /// alone, and what lies before is read from the store
    /// (`TranscriptStore`); right after it began anew, nothing. Of the
    /// prompt texts and the tool sets those the turn under way names
    /// (issue 383): its calls read them, and between turns nothing does;
    /// what an older message's context names is read from the store.
    pub fn keep_window(&mut self) {
        let last = self.history.end().saturating_sub(WINDOW);
        let turn = self.turn.as_ref().map(|turn| turn.work.turn_id);
        let began = turn.and_then(|turn| self.history.all_entries().find(|node| node.entry.said().is_some_and(|m| m.turn_id == Some(turn))).map(|node| node.seq));
        self.history.keep_from(began.map_or(last, |began| began.min(last)));
        self.history_from = self.history.base();
        let root = self.transcript.entries().next().map_or(self.transcript.end(), |node| node.seq);
        self.transcript.keep_from(root);
        self.transcript_from = self.transcript.base();
        let context = self.turn.as_ref().map(|turn| &turn.work.context);
        let named: HashSet<Digest> = context.into_iter().flat_map(|context| context.prompts.iter().map(|prompt| prompt.text)).collect();
        let offered = context.and_then(|context| context.tools);
        self.prompts.keep_only(|digest| named.contains(digest));
        self.offered_tools.keep_only(|digest| offered == Some(*digest));
    }

    /// Makes a loaded state fit to run from (issue 75). A state stored
    /// between two steps of a turn is a turn to resume - the server
    /// stopped or crashed in the middle of it - stored with the step that
    /// was to come next (issue 85), and most states resume as they are:
    /// `Planning` asks the model with the turn so far, `Reflecting` takes
    /// the finished turn over, `WaitingForUser` keeps waiting,
    /// `Cancelling` ends the turn. A deliberate end never leaves a turn in the
    /// store (`stop`, `cancel`, the stop tool).
    ///
    /// `ExecutingTools` is the exception: a tool may have run without its
    /// result being stored, and running it again would repeat its effect
    /// (issue 20). Every approved call without a result is answered as not
    /// executed, with `not_executed` - what the model is told of it -, so
    /// that the next step builds the tool message and the model decides
    /// again; results already stored stay. Returns how many calls were
    /// answered so.
    ///
    /// A state in the middle of a turn without the turn's working state
    /// cannot be resumed; it starts idle.
    pub fn prepare_resume(&mut self, not_executed: &str) -> usize {
        // A command turn is carried out between two stores and never stored
        // under way (issue 269); one found is dropped as its request would have
        // been, unanswered - and so is carrying it out (issue 299).
        if self.turn.as_ref().is_some_and(|turn| turn.command.is_some()) {
            warn!("A command turn was stored under way; it is dropped");
            self.turn = None;
        }
        if (self.processing_state.in_turn() && self.turn.is_none()) || self.processing_state == ActorState::CarryingOutCommand {
            self.processing_state = ActorState::Idle;
        }
        let Some(turn) = self.turn.as_mut().filter(|_| self.processing_state == ActorState::ExecutingTools) else {
            return 0;
        };
        let mut answered = 0;
        let classification = turn.work.classification.clone();
        for call in &mut turn.tool_calls {
            if matches!(call.status, ToolCallStatus::Ready { .. }) {
                call.status = ToolCallStatus::Finished(ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, not_executed.to_string(), classification.clone()));
                answered += 1;
            }
        }
        answered
    }

    /// Takes in what others said (issues 196, 206), the same for both kinds
    /// of agent: all that was offered to it, `heard`, into its history; what
    /// it perceived of that - what passed its perceive processors - into its
    /// transcript, which a turn under way has then seen (issue 168).
    pub fn perceive(&mut self, heard: Vec<Message>, perceived: Vec<Message>) {
        self.history.extend(heard);
        self.transcript.extend(perceived.clone());
        // A turn under way has seen it, and its next message is produced
        // from the transcript with it (issue 260).
        if let Some(turn) = self.turn.as_mut() {
            perceived.iter().for_each(|m| turn.work.see_message(m));
            turn.work.context.transcript = self.transcript.digest();
        }
    }

    /// Records `message`, said by the turn under way (issues 136, 260): the
    /// turn's from now on (`Message::turn_id`, issue 235), whoever built it,
    /// into the transcript and into the history with what it was produced
    /// from - the turn's context as it stands - and the context moved on
    /// past it. Kept as it is said, so that nothing of it waits for the
    /// turn's end. Once it ends a step - its calls have their results -,
    /// what the turn was given while they ran follows it (`given`, issue
    /// 389). Between turns, which says nothing, it is recorded as heard
    /// would be, without a context.
    pub fn said(&mut self, mut message: Message) {
        self.used = TokenUsage::counted(self.used, &message);
        let Some(turn) = self.turn.as_mut() else {
            tracing::warn!("A message was said by an agent without a turn under way");
            self.transcript.push(message.clone());
            self.history.push(message, None);
            return;
        };
        message.turn_id = Some(turn.work.turn_id);
        let context = turn.work.context.clone();
        self.transcript.push(message.clone());
        self.history.push(message, Some(context));
        turn.work.context.transcript = self.transcript.digest();
        if turn.tool_calls.is_empty() {
            for (message, knowledge) in mem::take(&mut turn.given_after_step) {
                self.given(message, knowledge);
            }
        }
    }

    /// Records `message`, given to the agent to read in the turn under way
    /// (issue 22) - a skill loaded for it - as the knowledge `knowledge`,
    /// if it is knowledge (issue 389): the turn's from now on
    /// (`Message::turn_id`), into the transcript as what it was given
    /// (`TranscriptEntry::Given`, issue 16) and into the history without a
    /// context - the agent did not produce it -, the knowledge recorded with
    /// the turn, and the turn's context moved on past it, so that what the
    /// turn says next is produced from it. Not published: it is what the
    /// agent read, not what it said. Between turns it is recorded as heard
    /// would be, still as given.
    ///
    /// While the calls of a step run, it is put aside and written after the
    /// step's tool message (`TurnState::given_after_step`, issue 389): a
    /// tool gives what it gives at any time, and nothing stands between the
    /// calls and their results, which the models refuse.
    pub fn given(&mut self, mut message: Message, knowledge: Option<KnowledgeRef>) {
        if let Some(turn) = self.turn.as_mut().filter(|turn| !turn.tool_calls.is_empty()) {
            turn.given_after_step.push((message, knowledge));
            return;
        }
        let source = knowledge.as_ref().map(|knowledge| knowledge.source);
        if let Some(turn) = self.turn.as_mut() {
            message.turn_id = Some(turn.work.turn_id);
            turn.work.see_message(&message);
            if let Some(knowledge) = knowledge {
                turn.work.record_knowledge(knowledge);
            }
        }
        self.transcript.give(message.clone(), source);
        self.history.push(message, None);
        if let Some(turn) = self.turn.as_mut() {
            turn.work.context.transcript = self.transcript.digest();
        }
    }

    /// Replaces the context of the turn under way of `actor` - the agent
    /// whose state this is - (issue 16): the transcript begins anew, as a
    /// branch of its own from the root, with `replacement` - what stands
    /// for the older messages: a summary, the skills given again, each
    /// given to read (`TranscriptEntry::Given`, issue 389) with the
    /// knowledge it is, if it is one - and after it the entries
    /// of the active path from `keep_from` on - `None`: none - as they
    /// stand, and after them where the compaction's part of the branch ends
    /// (`TranscriptEntry::Compacted`, issue 387). The turn's context moves
    /// to the new head, and the history records the compaction
    /// (`HistoryEntry::Compacted`), then the replacement as messages the
    /// agent was given. Returns the heads of the transcript before and
    /// after.
    ///
    /// Nothing is removed: the old branch stays, and every reference
    /// recorded before resolves to what it did. Of the entries kept, a
    /// message is copied as it stands - a change of it is in it, one taken
    /// back is left out - and a turn's end as it is, an earlier
    /// compaction's end not at all; every copy gets a digest of its own,
    /// since what comes before it is another. The
    /// replacement is seen by the turn; whose it is (`Message::turn_id`)
    /// is the caller's to say (issue 284). The
    /// prompt parts the turn placed after an entry are carried over (issue
    /// 284): one after an entry kept follows its copy - after a change of a
    /// message, which is in the copy, the copy before it -, one after an
    /// entry replaced follows the replacement.
    ///
    /// `None`, and nothing changes, without a turn under way, if the
    /// transcript is empty, if `keep_from` is not on its active path, or if
    /// the new branch would be empty.
    pub fn compact(&mut self, actor: ActorId, replacement: Vec<(Message, Option<KnowledgeRef>)>, keep_from: Option<Digest>) -> Option<(Digest, Digest)> {
        let turn = self.turn.as_mut()?;
        // A compaction comes before a call of the model, never while the
        // calls of a step run: nothing is put aside to be given (issue 389).
        debug_assert!(turn.given_after_step.is_empty());
        let from = self.transcript.digest()?;
        let path: Vec<_> = self.transcript.entries().collect();
        let start = match keep_from {
            None => path.len(),
            Some(first) => path.iter().position(|node| node.digest == first)?,
        };
        let replaced: Vec<Digest> = path[..start].iter().map(|node| node.digest).collect();
        // Each entry kept with its copy as it stands, `None` if none is made.
        let kept: Vec<(Digest, Option<TranscriptEntry>)> = as_they_stand(path[start..].iter().copied())
            .map(|(node, standing)| (node.digest, node.entry.copy_standing(standing)))
            .collect();
        if replacement.is_empty() && kept.iter().all(|(_, copy)| copy.is_none()) {
            return None;
        }
        self.transcript.start_anew();
        let mut given = Vec::new();
        for (message, knowledge) in replacement {
            turn.work.see_message(&message);
            // A summary written by the agent's model is a call of it.
            if message.sender_id == actor {
                self.used = TokenUsage::counted(self.used, &message);
            }
            self.transcript.give(message.clone(), knowledge.as_ref().map(|knowledge| knowledge.source));
            if let Some(knowledge) = knowledge {
                turn.work.record_knowledge(knowledge);
            }
            given.push(message);
        }
        // Where each entry of the old branch stands on the new one.
        let replacement_end = self.transcript.digest();
        let mut moved: HashMap<Digest, Option<Digest>> = replaced.into_iter().map(|old| (old, replacement_end)).collect();
        for (old, copy) in kept {
            if let Some(copy) = copy {
                self.transcript.record(copy);
            }
            moved.insert(old, self.transcript.digest());
        }
        self.transcript.record(TranscriptEntry::Compacted { at: Utc::now() });
        let to = self.transcript.digest()?;
        self.history.record(HistoryEntry::Compacted { turn_id: turn.work.turn_id, actor, from, to, at: Utc::now() });
        for message in given {
            self.history.push(message, None);
        }
        turn.work.context.transcript = Some(to);
        turn.work.context.prompts.retain_mut(|prompt| match prompt.after {
            None => true,
            Some(after) => moved.get(&after).map(|now| prompt.after = *now).is_some(),
        });
        // How full the new branch is: no call measured it yet, and the
        // summary's measured the old one. Estimated once, here, with what the
        // turn's model is given next (issue 390).
        let estimated = self.turn.as_ref()
            .and_then(|turn| ContextResolver::of(self).token_positions(&turn.work.context, turn.work.turn_id, actor))
            .and_then(|(positions, _)| positions.last().copied());
        if let (Some(used), Some(estimated)) = (self.used.as_mut(), estimated) {
            used.context_tokens = estimated;
        }
        Some((from, to))
    }

    /// What answering again instead of the message `id` of the agent `own`
    /// takes back (issue 234): the message, and what follows it on the
    /// active path of the transcript - which may only be the rest of its
    /// turn (`Message::turn_id`, issue 241), besides changes to what was
    /// said. So the message is its last answer, and the model is given the
    /// transcript as it stood before it, in what stands of it now. Of the
    /// ids returned, those its history holds are what it announces as taken
    /// back.
    ///
    /// # Errors
    /// Why it does not answer again: the message is not an answer of its
    /// own its model is still given, something else was said or recorded
    /// since, or what would be taken back holds calls of tools that ran -
    /// their effect stays, and the model would no longer know of it.
    pub fn retried(&self, id: MessageId, own: ActorId) -> Result<Vec<MessageId>, RetryRefused> {
        let mut path = as_they_stand(self.transcript.entries()).skip_while(|(_, m)| m.as_ref().is_none_or(|m| m.id != id));
        let Some((_, Some(message))) = path.next() else { return Err(RetryRefused::NotItsAnswer) };
        let Some(turn) = message.turn_id.filter(|_| message.sender_id == own && message.role == MessageRole::Assistant) else {
            return Err(RetryRefused::NotItsAnswer);
        };
        let mut taken_back = vec![message.id];
        for (node, standing) in path {
            match (&node.entry, standing) {
                (TranscriptEntry::Modified { .. } | TranscriptEntry::Revoked { .. } | TranscriptEntry::Reclassified { .. }, _) => {}
                // Its own turn's end.
                (TranscriptEntry::TurnFinished { turn_id, .. }, _) if *turn_id == turn => {}
                // Taken back already.
                (TranscriptEntry::Message(_), None) => {}
                (TranscriptEntry::Message(_), Some(step)) if step.turn_id == Some(turn) => {
                    if !executed_calls(&[&step]).is_empty() {
                        return Err(RetryRefused::CallsRan);
                    }
                    taken_back.push(step.id);
                }
                // Anything else since, a compaction too: the message may be
                // in a summary already (issue 387).
                (TranscriptEntry::Message(_) | TranscriptEntry::Given { .. } | TranscriptEntry::TurnFinished { .. } | TranscriptEntry::TurnGivenUp { .. } | TranscriptEntry::Compacted { .. }, _) => return Err(RetryRefused::NotLast),
            }
        }
        Ok(taken_back)
    }

    /// Every classification the remote agent of an external agent was
    /// given in this channel (issue 236), with those of what it is given
    /// now: every entry of the transcript's active branch that records
    /// something of a message, recorded as given
    /// (`RemoteState::give_classified`). Every branch before was the active
    /// one when its call was made - the transcript starts anew with each
    /// (`Transcript::start_anew`) -, so what it was given before is recorded
    /// already, and only the active branch is read (issue 376).
    pub fn classifications_given(&mut self) -> &[Classification] {
        for classification in self.transcript.entries().filter_map(|node| node.entry.classification()) {
            self.remote.give_classified(classification);
        }
        &self.remote.given_classifications
    }

    /// Keeps the text of a prompt part the agent was given once by digest
    /// (issue 47), and returns the reference to it: given in `role` after
    /// the entry `after` - `None`: before everything - its text classified
    /// `classification`.
    pub fn keep_prompt(&mut self, role: MessageRole, text: String, after: Option<Digest>, classification: Option<Classification>) -> PromptRef {
        let digest = digest::prompt(&text);
        self.prompts.keep(digest, || text);
        PromptRef { text: digest, role, after, classification }
    }

    /// Keeps the tool sets the agent's model is offered once by digest
    /// (issues 137, 194), and returns the digest that names the offer:
    /// what a `ContextRef` records; `None` for no sets, an offer of nothing.
    pub fn keep_offer(&mut self, tool_sets: Vec<ToolSet>) -> Option<Digest> {
        (!tool_sets.is_empty()).then(|| {
            let digest = digest::tool_sets(&tool_sets);
            self.offered_tools.keep(digest, || tool_sets);
            digest
        })
    }

    /// Begins a turn (issue 85): fixes what it is - the envelope it answers,
    /// whose authority it acts with, the channel's classification now and
    /// how the agent's model is given its context (the `profile`, issues
    /// 49, 63). What its model is offered is added once it is begun
    /// (`offer_tools`), since what may be called depends on the work.
    /// All but the first two hold for the whole turn - also one resumed after
    /// the model's configuration changed - so that what it reads, publishes
    /// and remembers is judged alike, and what its model is given too. The
    /// turn has seen the transcript, which is what the model is given first
    /// (issue 168). Whatever a previous turn left behind is gone. Returns the
    /// turn's id (issue 235).
    pub fn begin_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: Classification,
        profile: ModelProfile,
    ) -> TurnId {
        if self.turn.is_some() {
            tracing::warn!("A turn was begun while another was under way; the other ends without an end recorded");
        }
        let mut work = Work::new(event_context, security_context, classification);
        work.context = ContextRef { transcript: self.transcript.digest(), profile, ..ContextRef::default() };
        self.transcript.messages().iter().for_each(|m| work.see_message(m));
        let turn_id = work.turn_id;
        let turn = TurnState { work, ..TurnState::default() };
        self.turn = Some(turn);
        turn_id
    }

    /// Offers the model of the turn under way the tools in `tool_sets`
    /// (issues 137, 194, 290): kept once by digest and named by the turn's
    /// context. Once, right after the turn was begun and before its context
    /// is built - what it offers holds for the whole turn, also one resumed
    /// after the agent's tools changed. A turn offered tools already keeps
    /// them; without a turn, nothing is offered.
    pub fn offer_tools(&mut self, tool_sets: Vec<ToolSet>) {
        if self.turn.as_ref().is_none_or(|turn| turn.work.context.tools.is_some()) {
            tracing::warn!("Tools were offered with no turn under way, or to a turn offered tools already; nothing is offered");
            return;
        }
        let offered = self.keep_offer(tool_sets);
        if let Some(turn) = &mut self.turn {
            turn.work.context.tools = offered;
        }
    }

    /// Begins a turn that carries out `request` (issue 269): as `begin_turn`
    /// does, offering no tools - a command turn offers its model nothing -
    /// and the command kept with the turn, so that one resumed after a
    /// restart knows what to do.
    pub fn begin_command_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: Classification,
        profile: ModelProfile,
        request: CommandRequest,
    ) -> TurnId {
        let turn_id = self.begin_turn(event_context, security_context, classification, profile);
        if let Some(turn) = &mut self.turn {
            turn.command = Some(request);
        }
        turn_id
    }

    /// Whether the agent answers anew after its turn `turn`, given up (issue
    /// 259): the turn's end is the last thing on the active path of the
    /// transcript, and it was given up for a reason that may be answered
    /// again (`GivenUpReason::may_be_answered_again`). Nothing is taken
    /// back: what the turn said stays.
    ///
    /// # Errors
    /// `NotLast` if something followed the turn's end; `Ended` if it was
    /// given up for another reason, or taken over; `NotItsAnswer` if the
    /// transcript holds no end of that turn.
    pub fn answers_again_after(&self, turn: TurnId) -> Result<(), RetryRefused> {
        let ended = |entry: &TranscriptEntry| entry.ended_turn() == Some(turn);
        match self.transcript.last().map(|node| &node.entry) {
            Some(TranscriptEntry::TurnGivenUp { turn_id, reason, .. }) if *turn_id == turn => {
                if reason.may_be_answered_again() { Ok(()) } else { Err(RetryRefused::Ended) }
            }
            Some(entry) if ended(entry) => Err(RetryRefused::Ended),
            _ if self.transcript.entries().any(|node| ended(&node.entry)) => Err(RetryRefused::NotLast),
            _ => Err(RetryRefused::NotItsAnswer),
        }
    }

    /// Takes the turn `turn_id` over (issues 06, 17, 258): its changes to
    /// the agent's `notes` and `private_notes` here, anchored at its last
    /// message, and its end, `TurnFinished`, after what it said - which is
    /// in the transcript and the history already (`said`, issue 260).
    /// `actor` is the agent, whose turn it was. A turn that carried out a
    /// `command` (issue 269) records it in the history before its end.
    /// Returns whether there were changes to the notes.
    pub fn finish_turn(&mut self, turn_id: TurnId, actor: ActorId, notes: Vec<NoteChange>, private_notes: Vec<NoteChange>, command: Option<CommandRequest>) -> bool {
        // What branching (issue 08) will find the changes by: the turn's
        // last message, and the transcript as the turn left it.
        let after = self.history.last_message_id();
        let origin = NoteOrigin { transcript: self.transcript.digest() };
        let changed = !notes.is_empty() || !private_notes.is_empty();
        for change in notes {
            self.notes.push(change, origin.clone(), after);
        }
        for change in private_notes {
            self.private_notes.push(change, origin.clone(), after);
        }
        let at = Utc::now();
        if let Some(CommandRequest { command, args, issuer }) = command {
            self.history.record(HistoryEntry::CommandExecuted { turn_id, actor, issuer, command, args, at });
        }
        self.transcript.record(TranscriptEntry::TurnFinished { turn_id, at });
        self.history.record(HistoryEntry::TurnFinished { turn_id, actor, at });
        changed
    }

    /// Records that the turn `turn_id` was given up for `reason` (issue
    /// 258): its end, `TurnGivenUp`, after what it said, which stays - also
    /// for a turn that said nothing. What it staged is not applied.
    pub fn give_up_turn(&mut self, turn_id: TurnId, actor: ActorId, reason: GivenUpReason) {
        let at = Utc::now();
        self.transcript.record(TranscriptEntry::TurnGivenUp { turn_id, reason: reason.clone(), at });
        self.history.record(HistoryEntry::TurnGivenUp { turn_id, actor, reason, at });
    }

    /// The approvals its turn waits for (issue 172); none without a turn.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.turn.as_ref().map(TurnState::pending_approvals).unwrap_or_default()
    }

    /// The questions its turn waits for the answers to (issue 165); none
    /// without a turn.
    pub fn pending_questions(&self) -> Vec<QuestionRequest> {
        self.turn.as_ref().map(TurnState::pending_questions).unwrap_or_default()
    }
}

/// Why an agent does not answer again instead of one of its messages (issue
/// 234), as `ActorEvent::RetryRejected` tells it: a closed set, so that a
/// user interface can word and react to each itself. The text is for logs
/// and the TUI, after "… cannot answer again:".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum RetryRefused {
    #[error("it is not an answer of its own that it could give again")]
    NotItsAnswer,
    #[error("more was said or done since")]
    NotLast,
    #[error("calls of tools that ran would be taken back with it")]
    CallsRan,
    /// A later request, to respond or to answer again, came with it and is
    /// answered instead.
    #[error("it was asked again since")]
    AskedAgain,
    /// An external agent's remote side keeps what it was given (issue 205).
    #[error("it runs elsewhere and cannot take back what it was given")]
    Elsewhere,
    /// The answer asked for again while under way (issue 235) had ended
    /// when the request came: a cancel came first, or it failed.
    #[error("its answer had already ended")]
    Ended,
}

/// A change to an [`AgentChannelState`] since it was last stored (issue
/// 193): a variant per part of the state. A change of the agent's *state* -
/// what it heard, said, noted - as against an update of the agent itself.
/// The growing parts record what was added, the small ones are recorded
/// whole. The history and the transcript are chains a store keeps apart
/// (issues 379, 380).
// A record is made once per store; its size in memory does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChannelStateChange {
    Notes(Stored<LedgerEntry<Notes>>),
    PrivateNotes(Stored<LedgerEntry<Notes>>),
    ProcessingState(ActorState),
    /// The turn under way, whole: it is changed in place at every step
    /// (`prepare_resume`, the turn's own methods), and bounded by one turn.
    Turn(Option<TurnState>),
    /// What the agent's calls used, whole: a running total.
    Used(Option<TokenUsage>),
    /// What an external agent keeps of the remote side, whole: small, and
    /// bounded by its open tasks.
    Remote(RemoteState),
    /// From where the history is held, whole.
    HistoryFrom(u64),
    /// From where the transcript is held, whole.
    TranscriptFrom(u64),
}

impl Journaled for AgentChannelState {
    type Change = AgentChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        let mut changes: Vec<Self::Change> = Vec::new();
        changes.extend(self.notes.changes().map(AgentChannelStateChange::Notes));
        changes.extend(self.private_notes.changes().map(AgentChannelStateChange::PrivateNotes));
        changes.push(AgentChannelStateChange::ProcessingState(self.processing_state));
        changes.push(AgentChannelStateChange::Turn(self.turn.clone()));
        changes.push(AgentChannelStateChange::Used(self.used));
        changes.push(AgentChannelStateChange::Remote(self.remote.clone()));
        changes.push(AgentChannelStateChange::HistoryFrom(self.history_from));
        changes.push(AgentChannelStateChange::TranscriptFrom(self.transcript_from));
        changes
    }

    /// The history, the transcript, the prompt parts and the tool sets are
    /// marked stored by the store that wrote them.
    fn mark_stored(&mut self) {
        self.notes.mark_stored();
        self.private_notes.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            AgentChannelStateChange::Notes(change) => self.notes.apply(change)?,
            AgentChannelStateChange::PrivateNotes(change) => self.private_notes.apply(change)?,
            AgentChannelStateChange::ProcessingState(state) => self.processing_state = state,
            AgentChannelStateChange::Turn(turn) => self.turn = turn,
            AgentChannelStateChange::Used(used) => self.used = used,
            AgentChannelStateChange::Remote(remote) => self.remote = remote,
            AgentChannelStateChange::HistoryFrom(from) => self.history_from = from,
            AgentChannelStateChange::TranscriptFrom(from) => self.transcript_from = from,
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
    use crate::history::testing::{items_of, messages_of, recorded_context};
    use crate::actor::ActorState;
    use crate::confidentiality::Confidentiality;
    use crate::context::{ContextResolver, KnowledgeSource};
    use crate::digest;
    use crate::store::Stored;
    use crate::tool::{Tool, ToolParameters, ToolVisibility};
    use crate::user::UserId;
    use crate::workspace::{Workspace, WorkspaceId};
    use crate::note::NoteId;

    #[test]
    fn a_resumed_turn_answers_unfinished_tool_calls_as_not_executed_instead_of_running_them() {
        use crate::approval::ToolMatcher;
        use crate::tool::{ToolCall, ToolCallState};
        let tool_call = ToolCall { id: None, function_name: "write_note".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let ready = ToolCallState::approved(tool_call.clone());
        let done = ToolCallState::answered(tool_call.clone(), ToolCallResult::of(&tool_call, ToolResultStatus::Ok, "done".to_string(), Classification::CHANNEL_DEFAULT));
        let waiting = ToolCallState::pending(tool_call.clone(), String::new(), false, ToolMatcher::exact(&tool_call), None);
        let state_in = |actor_state: ActorState| AgentChannelState {
            processing_state: actor_state,
            turn: Some(TurnState {
                tool_calls: vec![ready.clone(), done.clone(), waiting.clone()],
                ..Default::default()
            }),
            ..AgentChannelState::default()
        };
        let calls = |state: &AgentChannelState| state.turn.as_ref().map(|t| t.tool_calls.clone()).unwrap_or_default();

        let mut executing = state_in(ActorState::ExecutingTools);
        assert_eq!(executing.prepare_resume("not executed"), 1, "one approved call had no result");
        let executing = calls(&executing);
        let [unfinished, finished, pending] = executing.as_slice() else { panic!("three calls") };
        let unfinished = unfinished.result().expect("answered");
        assert_eq!(unfinished.status, ToolResultStatus::NotExecuted);
        assert_eq!(unfinished.content, "not executed", "the model is told what it was given");
        assert_eq!(finished.result().map(|r| r.content.as_str()), Some("done"), "a stored result stays");
        assert!(pending.approval_id().is_some(), "a call nobody approved is still waiting");

        // Every other state resumes as it is.
        for other in [ActorState::Planning, ActorState::WaitingForUser, ActorState::Reflecting] {
            let mut state = state_in(other);
            assert_eq!(state.prepare_resume("not executed"), 0, "{other:?}");
            assert!(matches!(calls(&state)[0].status, ToolCallStatus::Ready { .. }));
            assert_eq!(state.processing_state, other);
        }

        // A turn state without the turn cannot be resumed.
        let mut lost = AgentChannelState { processing_state: ActorState::Planning, ..AgentChannelState::default() };
        lost.prepare_resume("not executed");
        assert_eq!(lost.processing_state, ActorState::Idle);

        // A command turn is never resumed, nor carrying it out.
        let mut command = AgentChannelState {
            processing_state: ActorState::CarryingOutCommand,
            turn: Some(TurnState { command: Some(CommandRequest { command: "mark".to_string(), args: String::new(), issuer: ActorId::None }), ..Default::default() }),
            ..AgentChannelState::default()
        };
        command.prepare_resume("not executed");
        assert!(command.turn.is_none());
        assert_eq!(command.processing_state, ActorState::Idle);
    }

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

    /// Issue 341: an agent acting when nobody asked it is the caller and
    /// the initiator - it caused what it does -, answered for by its owner
    /// and acting in its owner's name.
    #[test]
    fn an_agent_acting_on_its_own_initiates_it_for_its_owner() {
        let owner = ActorId::User(UserId::new());
        let agent = Agent::new(ScopedName::new("helper", Default::default()).unwrap(), PersonalityId::new(), owner);
        let context = agent.security_context(Default::default(), Default::default());
        let itself = ActorId::Agent(agent.id);
        assert_eq!((context.caller_id, context.initiator_id), (itself, itself), "it acts, and caused it");
        assert_eq!((context.owner_id, context.principal_id), (owner, owner), "answered for by its owner, in their name");
    }

    /// Issue 341: an agent is made acting in its owner's name, and cannot
    /// be stored acting in anybody else's - whom else it may act for is
    /// issue 340.
    #[test]
    fn an_agent_acts_in_its_owners_name() {
        let owner = ActorId::User(UserId::new());
        let mut agent = Agent::new(ScopedName::new("helper", Default::default()).unwrap(), PersonalityId::new(), owner);
        assert_eq!(agent.principal_id, owner, "its owner's when made");
        assert!(agent.validate().is_ok());
        agent.principal_id = ActorId::User(UserId::new());
        assert!(agent.validate().is_err(), "not lent another's rights");
    }

    #[test]
    fn only_an_agent_run_here_can_retry() {
        assert!(PersonalityKind::Internal.can_retry());
        assert!(!PersonalityKind::External.can_retry(), "its remote side keeps what it was given");
    }

    #[test]
    fn a_personality_limits_its_agents_only_where_it_names_roles_unless_external() {
        let mut personality: Personality = serde_json::from_value(serde_json::json!({ "id": PersonalityId::new(), "name": "plain" })).unwrap();
        assert_eq!(personality.limiting_roles(), None, "internal, none named: the owner's alone");
        personality.kind = PersonalityKind::External;
        assert_eq!(personality.limiting_roles(), Some(&[][..]), "external, none named: public only");
        let staff = RoleId::new();
        personality.roles = Some(vec![staff]);
        assert_eq!(personality.limiting_roles(), Some(&[staff][..]));
        personality.kind = PersonalityKind::Internal;
        assert_eq!(personality.limiting_roles(), Some(&[staff][..]), "named, internal too");
    }

    #[test]
    fn a_turn_under_way_is_stored_with_the_state_and_none_between_turns() {
        let idle = AgentChannelState::default();
        let json = serde_json::to_string(&idle).unwrap();
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(decoded.turn.is_none() && decoded.processing_state == ActorState::Idle);

        let mut waiting = AgentChannelState { processing_state: ActorState::WaitingForUser, turn: Some(TurnState::default()), ..AgentChannelState::default() };
        waiting.said(Message::new(Classification::CHANNEL_DEFAULT));
        let json = serde_json::to_string(&waiting).unwrap();
        assert!(json.contains("\"WaitingForUser\""));
        let mut decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(decoded.transcript.is_empty(), "the transcript is a store's to keep apart (issue 380)");
        decoded.transcript = waiting.transcript.clone();
        assert_eq!(decoded.processing_state, ActorState::WaitingForUser);
        let turn_id = decoded.turn.as_ref().map(|t| t.work.turn_id).expect("the turn is resumed from what was stored");
        assert_eq!(decoded.transcript.messages().len(), 1, "with what it said, in the transcript (issue 260)");
        assert_eq!(decoded.turn.as_ref().map(|t| t.work.context.transcript), Some(decoded.transcript.digest()), "its context names the transcript's head");
        let context = decoded.turn.as_ref().map(|t| t.work.context.clone()).unwrap_or_default();
        assert!(ContextResolver::of(&decoded).resolve(&context, turn_id, ActorId::default()).is_some(), "so a resumed turn's next call resolves");
        assert_eq!(decoded.turn.map(|t| t.work.turn_id), waiting.turn.map(|t| t.work.turn_id), "as the same turn");
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

    /// Begins a turn offering `tools` as one set, or none.
    fn begin(state: &mut AgentChannelState, tools: Vec<Tool>) {
        let sets = if tools.is_empty() { Vec::new() } else { vec![ToolSet::new("tools", "some tools", tools)] };
        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT, ModelProfile::default());
        state.offer_tools(sets);
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
            presentation: Default::default(),
        };
        begin(&mut state, vec![tool.clone()]);
        begin(&mut state, vec![tool]);
        let turn = state.turn.as_ref().expect("a turn was begun");
        assert_eq!(turn.work.label(), internal, "it has seen the transcript");
        assert_eq!(turn.work.context.transcript, state.transcript.digest(), "it continues the transcript");
        assert!(turn.work.context.tools.is_some() && state.offered_tools.len() == 1, "the set offered is kept once");
        let offered = turn.work.context.tools;
        state.offer_tools(vec![ToolSet::new("more", "more tools", Vec::new())]);
        assert_eq!(state.turn.as_ref().and_then(|t| t.work.context.tools), offered, "a turn offered tools keeps them");

        begin(&mut state, Vec::new());
        assert!(state.turn.as_ref().is_some_and(|t| t.work.context.tools.is_none()), "no tools, no set");
    }

    #[test]
    fn a_turn_says_into_the_transcript_and_the_history_and_ends_finished() {
        let mut state = a_state_that_heard("add 1 and 2", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        let turn_id = state.turn.as_ref().expect("a turn was begun").work.turn_id;
        state.said(Message { internal: true, ..said("thinking", Classification::CHANNEL_DEFAULT) });
        let produced_from = state.turn.as_ref().unwrap().work.context.clone();
        let answer = said("3", Classification::CHANNEL_DEFAULT);
        assert!(answer.turn_id.is_none(), "built without it");
        state.said(answer.clone());
        assert_eq!(produced_from.transcript, state.transcript.entries().nth(1).map(|n| n.digest), "said after the step, from the transcript with it");
        assert_eq!(state.turn.as_ref().unwrap().work.context.transcript, state.transcript.digest(), "the context moved on to the head");
        assert_eq!(recorded_context(&state.history, answer.id), Some((produced_from.clone(), turn_id)), "recorded as it is said (issue 260)");
        let note = NoteChange::Add { id: NoteId(1), text: "sums".to_string(), classification: Classification::CHANNEL_DEFAULT };

        state.turn = None;
        assert!(state.finish_turn(turn_id, ActorId::default(), vec![note], Vec::new(), None), "a note changed");
        assert_eq!(state.transcript.entries().count(), 4, "the turn's messages, followed by its end");
        assert!(matches!(state.transcript.last().map(|n| &n.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if *t == turn_id));
        assert!(matches!(state.history.last().map(|n| &n.entry), Some(HistoryEntry::TurnFinished { turn_id: t, .. }) if *t == turn_id));
        assert_eq!(messages_of(&state.history).iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>(), ["add 1 and 2", "thinking", "3"], "an internal step too (issue 258)");
        let turns: Vec<_> = state.transcript.messages().iter().map(|m| m.turn_id).collect();
        assert_eq!(turns, [None, Some(turn_id), Some(turn_id)], "what the turn keeps is the turn's, what was heard no turn's");
        assert_eq!(messages_of(&state.history).last().and_then(|m| m.turn_id), Some(turn_id));
        assert_eq!(recorded_context(&state.history, answer.id), Some((produced_from, turn_id)), "with what it was produced from, and in which turn");
        assert_eq!(state.notes.current().len(), 1);
    }

    #[test]
    fn a_message_given_in_a_turn_is_read_not_said() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let turn_id = state.turn.as_ref().unwrap().work.turn_id;
        let skill = Message { role: MessageRole::User, ..said("Collect, group, write.", Classification::PUBLIC) };
        let source = KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), Some(digest::prompt("Collect, group, write.")));
        let knowledge = KnowledgeRef { channel: Default::default(), session: Default::default(), source, classification: Some(Classification::CHANNEL_DEFAULT) };

        state.given(skill.clone(), Some(knowledge.clone()));
        assert_eq!(state.transcript.messages().last().map(|m| (m.id, m.turn_id)), Some((skill.id, Some(turn_id))), "into the transcript, the turn's");
        assert!(matches!(state.transcript.last().map(|n| &n.entry), Some(TranscriptEntry::Given { source: Some(s), .. }) if *s == source), "as what it was given (issue 16)");
        let work = &state.turn.as_ref().unwrap().work;
        assert_eq!(work.context.knowledge, [knowledge], "the knowledge recorded with the turn");
        assert_eq!(work.label(), Classification::CHANNEL_DEFAULT, "which has seen it");
        assert_eq!(state.turn.as_ref().unwrap().work.context.transcript, state.transcript.digest(), "what the turn says next is produced from it");
        assert_eq!(recorded_context(&state.history, skill.id), None, "the agent did not produce it");
        assert_eq!(messages_of(&state.history).last().map(|m| m.id), Some(skill.id), "and its history keeps it, as what it was given");
    }

    #[test]
    fn what_is_given_while_the_calls_of_a_step_run_follows_their_results() {
        use crate::tool::{ToolCall, ToolCallState};
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let call = ToolCall { id: Some("1".to_string()), function_name: "load_skill".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        state.said(Message { role: MessageRole::Assistant, tool_calls: vec![call.clone()], ..said("", Classification::PUBLIC) });
        state.turn.as_mut().unwrap().tool_calls = vec![ToolCallState::approved(call.clone())];
        let skill = said("Collect, group, write.", Classification::PUBLIC);
        let summary = said("what was given without being knowledge", Classification::PUBLIC);

        state.given(skill.clone(), Some(KnowledgeRef { channel: Default::default(), session: Default::default(), source: KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), None), classification: None }));
        state.given(summary.clone(), None);
        assert_eq!(state.transcript.messages().len(), 2, "nothing between the calls and their results");

        let result = ToolCallResult::of(&call, ToolResultStatus::Ok, "loaded".to_string(), Classification::PUBLIC);
        state.turn.as_mut().unwrap().tool_calls.clear();
        let results = Message { role: MessageRole::Tool, tool_results: vec![result], ..said("", Classification::PUBLIC) };
        state.said(results.clone());
        let order: Vec<_> = state.transcript.messages().iter().map(|m| m.id).collect();
        assert_eq!(order[2..], [results.id, skill.id, summary.id], "after the results, in the order given");
        let sources: Vec<_> = state.transcript.entries().skip(3).map(|n| matches!(&n.entry, TranscriptEntry::Given { source: Some(_), .. })).collect();
        assert_eq!(sources, [true, false], "each as it was given, knowledge or not");
        assert!(state.turn.as_ref().unwrap().given_after_step.is_empty());
        assert_eq!(state.turn.as_ref().unwrap().work.context.transcript, state.transcript.digest(), "what the turn says next is produced from it");
    }

    #[test]
    fn a_compaction_begins_the_transcript_anew_and_keeps_what_was_recorded_resolvable() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let answer = Message { sender_id: agent, role: MessageRole::Assistant, prompt_tokens: Some(100), completion_tokens: Some(10), ..said("hi", Classification::PUBLIC) };
        state.said(answer.clone());
        let first = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(first, agent, Vec::new(), Vec::new(), None);
        let answered = state.transcript.entries().find(|n| n.entry.message().is_some_and(|m| m.id == answer.id)).unwrap().digest;
        let (recorded, turn) = recorded_context(&state.history, answer.id).expect("recorded with the answer");
        let resolve = |state: &AgentChannelState| ContextResolver::of(state).resolve(&recorded, turn, agent).map(|r| r.items.len());
        let before = resolve(&state);
        let (again, gone) = (said("again", Classification::PUBLIC), said("gone", Classification::PUBLIC));
        state.perceive(vec![again.clone(), gone.clone()], vec![again.clone(), gone.clone()]);
        state.transcript.modify(Message { content: Some("again!".to_string()), ..again.clone() });
        state.transcript.revoke(gone.id, Utc::now());
        let head = state.transcript.digest();

        assert_eq!(state.compact(agent, vec![(said("summary", Classification::PUBLIC), None)], None), None, "not between turns");
        begin(&mut state, Vec::new());
        assert_eq!(state.compact(agent, Vec::new(), None), None, "not to nothing");
        assert_eq!(state.compact(agent, Vec::new(), Some(digest::content(b"elsewhere"))), None, "not from what is not on the active path");
        assert_eq!(state.transcript.digest(), head, "nothing changed");

        let summary = Message { sender_id: agent, role: MessageRole::User, prompt_tokens: Some(50), ..said("[Summary] they greeted", Classification::PUBLIC) };
        let source = KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), Some(digest::prompt("steps")));
        let knowledge = KnowledgeRef { channel: Default::default(), session: Default::default(), source, classification: Some(Classification::CHANNEL_DEFAULT) };
        let skill = said("[Skill: steps]", Classification::PUBLIC);
        let (from, to) = state.compact(agent, vec![(summary.clone(), None), (skill.clone(), Some(knowledge.clone()))], Some(answered)).expect("compacted");

        assert_eq!(Some(from), head, "from what stood");
        assert_eq!(state.transcript.digest(), Some(to));
        let entries: Vec<String> = state.transcript.entries().map(|n| match &n.entry {
            TranscriptEntry::Message(m) => format!("said {}", m.content.clone().unwrap_or_default()),
            TranscriptEntry::Given { message, .. } => format!("given {}", message.content.clone().unwrap_or_default()),
            TranscriptEntry::TurnFinished { turn_id, .. } => format!("finished {}", *turn_id == first),
            TranscriptEntry::Compacted { .. } => "compacted".to_string(),
            other => format!("{other:?}"),
        }).collect();
        assert_eq!(entries, ["given [Summary] they greeted", "given [Skill: steps]", "said hi", "finished true", "said again!", "compacted"],
            "the replacement, given (issue 389), then what was kept as it stands - the change in it, the message taken back left out -, then where the compaction's part ends (issue 387)");
        let turn_under_way = &state.turn.as_ref().unwrap().work;
        assert_eq!(turn_under_way.context.transcript, Some(to), "what the turn says next is produced from the new branch");
        assert_eq!(turn_under_way.context.knowledge, [knowledge], "the skill given again is knowledge of the turn");
        let sources: Vec<_> = state.transcript.entries().take(2).map(|n| match &n.entry { TranscriptEntry::Given { source, .. } => *source, _ => None }).collect();
        assert_eq!(sources, [None, Some(source)], "the summary is no knowledge, the skill is");
        assert!(state.transcript.messages().iter().take(2).all(|m| m.turn_id.is_none()), "the replacement is whose its caller says (issue 284)");

        assert_eq!(resolve(&state), before, "what was recorded before resolves to what it did");
        assert!(state.transcript.prefix(&head.unwrap()).is_some(), "the old branch stays");
        let items = items_of(&state.history);
        let compacted = items.iter().position(|item| matches!(item, HistoryEntry::Compacted { from: f, to: t, actor, .. } if *f == from && *t == to && *actor == agent)).expect("recorded");
        let followed: Vec<_> = items[compacted + 1..].iter().filter_map(|item| match item { HistoryEntry::Said { message, context: None } => Some(message.id), _ => None }).collect();
        assert_eq!(followed, [summary.id, skill.id], "then what replaced the older messages, as given");
        let usage = state.used.expect("calls were made");
        assert_eq!((usage.calls, usage.prompt_tokens), (2, 150), "the answer kept counts once");

        let again = state.transcript.entries().find(|n| n.entry.message().is_some_and(|m| m.content.as_deref() == Some("again!"))).map(|n| n.digest);
        state.compact(agent, Vec::new(), again).expect("compacted again");
        let ends = state.transcript.entries().filter(|n| matches!(n.entry, TranscriptEntry::Compacted { .. })).count();
        assert_eq!(ends, 1, "an earlier compaction's end is not kept: the copies before it are the new one's");
        assert!(matches!(state.transcript.last().map(|n| &n.entry), Some(TranscriptEntry::Compacted { .. })));
    }

    #[test]
    fn after_a_compaction_the_context_is_the_new_branch_as_estimated_then() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let first = Message { sender_id: agent, role: MessageRole::Assistant, prompt_tokens: Some(100), completion_tokens: Some(10), ..said("call", Classification::PUBLIC) };
        state.said(first.clone());
        let kept_from = state.transcript.entries().find(|n| n.entry.message().is_some_and(|m| m.id == first.id)).map(|n| n.digest);
        let summary = Message { sender_id: agent, prompt_tokens: Some(500), completion_tokens: Some(8), ..said("summary", Classification::PUBLIC) };
        state.compact(agent, vec![(summary, None)], kept_from).expect("compacted");
        assert_eq!(state.used.map(|u| (u.calls, u.context_tokens)), Some((2, 2 + 1)), "the new branch as estimated, not the summary's call over the old one");

        let turn = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(turn, agent, Vec::new(), Vec::new(), None);
        assert_eq!(state.used.map(|u| u.context_tokens), Some(3), "kept, between turns as well (issue 390)");
    }

    #[test]
    fn a_compaction_carries_the_prompt_parts_of_the_turn_over() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("older", Classification::PUBLIC);
        let older = state.transcript.digest();
        let kept = said("kept", Classification::PUBLIC);
        let changed = said("changed", Classification::PUBLIC);
        state.perceive(vec![kept.clone(), changed.clone()], vec![kept.clone(), changed.clone()]);
        let kept_at = state.transcript.entries().find(|n| n.entry.message().is_some_and(|m| m.id == kept.id)).map(|n| n.digest);
        state.transcript.modify(Message { content: Some("changed!".to_string()), ..changed.clone() });
        let change_at = state.transcript.digest();
        begin(&mut state, Vec::new());
        let parts = [("first", None), ("on the replaced", older), ("on the kept", kept_at), ("on a change", change_at)]
            .map(|(text, after)| state.keep_prompt(MessageRole::System, text.to_string(), after, None));
        state.turn.as_mut().unwrap().work.context.prompts = parts.to_vec();

        state.compact(agent, vec![(said("summary", Classification::PUBLIC), None)], kept_at).expect("compacted");

        let on_new: Vec<Option<String>> = state.transcript.entries().map(|n| n.entry.message().and_then(|m| m.content.clone())).collect();
        let anchor = |prompt: &PromptRef| prompt.after.map(|after| state.transcript.entries().position(|n| n.digest == after).map(|at| on_new[at].clone()));
        let anchors: Vec<_> = state.turn.as_ref().unwrap().work.context.prompts.iter().map(anchor).collect();
        assert_eq!(anchors, [
            None,
            Some(Some(Some("summary".to_string()))),
            Some(Some(Some("kept".to_string()))),
            Some(Some(Some("changed!".to_string()))),
        ], "before everything as it was, after the replacement for what it replaced, after the copy for what was kept - a change's after its message");
        let context = state.turn.as_ref().unwrap().work.context.clone();
        assert!(ContextResolver::of(&state).resolve(&context, TurnId::new(), agent).is_some(), "every part is placed on the new branch");
    }

    #[test]
    fn a_command_turn_offers_no_tools_and_records_its_command_before_its_end() {
        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        let issuer = ActorId::User(UserId::new());
        let request = CommandRequest { command: "compact".to_string(), args: "hard".to_string(), issuer };
        let turn_id = state.begin_command_turn(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT, ModelProfile::default(), request.clone());
        let turn = state.turn.as_ref().expect("a turn was begun");
        assert_eq!(turn.command.as_ref(), Some(&request), "kept with the turn, for one resumed");
        assert!(turn.work.context.tools.is_none(), "a command turn offers its model nothing");

        state.turn = None;
        let agent = ActorId::Agent(AgentId::new());
        assert!(!state.finish_turn(turn_id, agent, Vec::new(), Vec::new(), Some(request)));
        let ends: Vec<String> = items_of(&state.history).iter().filter_map(|item| match item {
            HistoryEntry::CommandExecuted { turn_id: t, actor, issuer: by, command, args, .. } if *t == turn_id => Some(format!("{actor} ran /{command} {args} for {by}")),
            HistoryEntry::TurnFinished { turn_id: t, .. } if *t == turn_id => Some("finished".to_string()),
            _ => None,
        }).collect();
        assert_eq!(ends, [format!("{agent} ran /compact hard for {issuer}"), "finished".to_string()], "the command before the turn's end (issue 269)");
        assert!(matches!(state.transcript.last().map(|n| &n.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if *t == turn_id));
        assert_eq!(messages_of(&state.history).len(), 1, "nothing said");
    }

    #[test]
    fn a_turn_given_up_keeps_what_it_said_and_ends_given_up() {
        let mut state = a_state_that_heard("mail bob", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        state.said(said("mailing", Classification::CHANNEL_DEFAULT));

        let turn_id = state.turn.take().expect("a turn was begun").work.turn_id;
        state.give_up_turn(turn_id, ActorId::default(), GivenUpReason::Left);
        let contents = |messages: Vec<Message>| messages.iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>();
        assert_eq!(contents(state.transcript.messages()), ["mail bob", "mailing"], "what it said is not unsaid");
        assert_eq!(contents(messages_of(&state.history)), ["mail bob", "mailing"]);
        assert!(matches!(state.history.last().map(|n| &n.entry), Some(HistoryEntry::TurnGivenUp { turn_id: t, reason: GivenUpReason::Left, .. }) if *t == turn_id));
        assert_eq!(state.transcript.all_entries().count(), state.transcript.entries().count(), "nothing branches");
        assert!(matches!(state.transcript.last().map(|n| &n.entry), Some(TranscriptEntry::TurnGivenUp { turn_id: t, .. }) if *t == turn_id));

        let silent = TurnId::new();
        state.give_up_turn(silent, ActorId::default(), GivenUpReason::Left);
        assert!(matches!(state.history.last().map(|n| &n.entry), Some(HistoryEntry::TurnGivenUp { turn_id: t, .. }) if *t == silent), "a turn that said nothing leaves its end too");
    }

    #[test]
    fn an_agent_answers_anew_after_its_last_turn_given_up_for_a_failure_or_a_lowering() {
        let given_up = |reason: GivenUpReason| {
            let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
            begin(&mut state, Vec::new());
            let turn = state.turn.take().expect("a turn was begun").work.turn_id;
            state.give_up_turn(turn, ActorId::default(), reason);
            (state, turn)
        };
        let (state, turn) = given_up(GivenUpReason::Failed { message: "boom".to_string() });
        assert_eq!(state.answers_again_after(turn), Ok(()), "a turn that failed, though it said nothing");
        assert_eq!(state.answers_again_after(TurnId::new()), Err(RetryRefused::NotItsAnswer), "no turn of its own");
        let lowered = given_up(GivenUpReason::Lowered { channel: Classification::PUBLIC });
        assert_eq!(lowered.0.answers_again_after(lowered.1), Ok(()));
        for reason in [GivenUpReason::Left, GivenUpReason::Retried] {
            let (state, turn) = given_up(reason.clone());
            assert_eq!(state.answers_again_after(turn), Err(RetryRefused::Ended), "{reason:?}");
        }

        let (mut state, turn) = given_up(GivenUpReason::Failed { message: "boom".to_string() });
        let later = said("are you there?", Classification::CHANNEL_DEFAULT);
        state.perceive(vec![later.clone()], vec![later]);
        assert_eq!(state.answers_again_after(turn), Err(RetryRefused::NotLast), "something followed it");

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        let turn = state.turn.take().expect("a turn was begun").work.turn_id;
        state.finish_turn(turn, ActorId::default(), Vec::new(), Vec::new(), None);
        assert_eq!(state.answers_again_after(turn), Err(RetryRefused::Ended), "taken over, not given up");
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
    fn an_agent_keeps_its_history_from_its_turn_under_way_and_at_least_the_last_entries() {
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.history.extend((1..=300).map(|n| make_message(id(n))).collect());
        state.history.mark_stored();
        state.keep_window();
        assert_eq!((state.history.base(), state.history_from), (100, 100), "never compacted: the last entries alone (issue 387)");
        state.history.record(HistoryEntry::Compacted { turn_id: TurnId::new(), actor: ActorId::default(), from: digest::prompt("before"), to: digest::prompt("after"), at: Utc::now() });
        state.history.extend((301..=600).map(|n| make_message(id(n))).collect());
        state.history.mark_stored();
        state.keep_window();
        assert_eq!(state.history.base(), 401, "a compaction does not keep what came after it");

        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT, ModelProfile::default());
        let turn = state.turn.as_ref().unwrap().work.turn_id;
        let first = Message { turn_id: Some(turn), ..make_message(id(601)) };
        state.history.push(first.clone(), Some(ContextRef::default()));
        state.history.extend((602..=900).map(|n| make_message(id(n))).collect());
        state.history.mark_stored();
        state.keep_window();
        assert_eq!(state.history.base(), 601, "from the first message of the turn under way, more than the last entries");
        assert_eq!(state.history.said_in_turn(ActorId::default(), turn).iter().map(|m| m.id).collect::<Vec<_>>(), [first.id], "what the turn said");

        state.turn = None;
        state.keep_window();
        assert_eq!(state.history.base(), 701, "between turns, the last entries");
    }

    #[test]
    fn an_agent_keeps_the_active_branch_of_its_transcript() {
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.transcript.extend((1..=3).map(|n| make_message(id(n))).collect());
        state.transcript.mark_stored();
        state.keep_window();
        assert_eq!((state.transcript.base(), state.transcript_from), (0, 0), "one branch: all of it");
        state.transcript.extend(vec![make_message(id(4))]);
        let given = state.transcript.start_anew().expect("a branch");
        state.transcript.extend(vec![make_message(id(5))]);
        state.keep_window();
        assert_eq!(state.transcript.base(), 0, "nothing goes before it is stored");
        state.transcript.mark_stored();
        state.keep_window();
        assert_eq!((state.transcript.base(), state.transcript_from, state.transcript.messages().len()), (4, 4, 1), "from the root of the active branch");
        assert!(state.transcript.prefix(&given).is_none(), "the branch before is the store's");
        state.transcript.start_anew();
        state.transcript.mark_stored();
        state.keep_window();
        assert_eq!((state.transcript.base(), state.transcript.len()), (5, 0), "right after it began anew, nothing");
    }

    #[test]
    fn an_agent_keeps_the_texts_and_tools_its_turn_under_way_names() {
        let mut state = AgentChannelState::default();
        let earlier = state.keep_prompt(MessageRole::System, "earlier".to_string(), None, None);
        let sets = vec![ToolSet::new("calc", "calculates", Vec::new())];
        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT, ModelProfile::default());
        state.offer_tools(sets);
        let system = state.keep_prompt(MessageRole::System, "system".to_string(), None, None);
        state.turn.as_mut().unwrap().work.context.prompts.push(system.clone());
        let offer = state.turn.as_ref().unwrap().work.context.tools.expect("an offer");
        state.keep_window();
        assert_eq!((state.prompts.len(), state.offered_tools.len()), (2, 1), "nothing goes before it is stored");

        state.prompts.mark_stored();
        state.offered_tools.mark_stored();
        state.keep_window();
        assert!(state.prompts.get(&earlier.text).is_none(), "what no turn under way names is the store's");
        assert!(state.prompts.get(&system.text).is_some() && state.offered_tools.get(&offer).is_some(), "what it names stays");

        state.turn = None;
        state.keep_window();
        assert!(state.prompts.is_empty() && state.offered_tools.is_empty(), "between turns, nothing");
    }

    #[test]
    fn what_changed_in_an_agents_channel_state_replays_onto_what_was_stored() {
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.history.extend(vec![make_message(id(1))]);
        state.transcript.push(make_message(id(1)));
        let mut copy = stored(&mut state);

        let before = state.transcript.push(make_message(id(2)));
        state.history.push(make_message(id(2)), Some(ContextRef { transcript: before, ..ContextRef::default() }));
        state.notes.push(NoteChange::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, Default::default(), id(2));
        state.private_notes.push(NoteChange::Add { id: NoteId(1), text: "mine".to_string(), classification: Classification::CHANNEL_DEFAULT }, Default::default(), id(2));
        state.processing_state = ActorState::WaitingForUser;
        state.turn = Some(TurnState { model_calls: 3, ..TurnState::default() });
        state.used = TokenUsage::counted(None, &Message { prompt_tokens: Some(5), ..make_message(id(2)) });

        let changes = state.changes();
        let parts: Vec<&str> = changes.iter().map(|c| match c {
            AgentChannelStateChange::Notes(_) => "notes",
            AgentChannelStateChange::PrivateNotes(_) => "private_notes",
            AgentChannelStateChange::ProcessingState(_) => "processing_state",
            AgentChannelStateChange::Turn(_) => "turn",
            AgentChannelStateChange::Used(_) => "used",
            AgentChannelStateChange::Remote(_) => "remote",
            AgentChannelStateChange::HistoryFrom(_) => "history_from",
            AgentChannelStateChange::TranscriptFrom(_) => "transcript_from",
        }).collect();
        assert_eq!(parts, ["notes", "private_notes", "processing_state", "turn", "used", "remote", "history_from", "transcript_from"], "what was added, then the small parts whole; the history, the transcript, the prompt parts and the tool sets are the store's to write");
        replayed(&state, &mut copy);
        assert_eq!(copy.used, state.used, "the running total with them");

        state.mark_stored();
        state.turn = None;
        assert_eq!(state.changes().len(), 6, "between turns a step is the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.turn.is_none());
    }

    #[test]
    fn all_that_is_heard_is_kept_and_what_is_perceived_is_seen_by_a_turn_under_way() {
        use crate::message::tests::{id, make_message};
        let mut state = AgentChannelState::default();
        state.perceive(vec![make_message(id(1)), make_message(id(2))], vec![make_message(id(1))]);
        assert_eq!(messages_of(&state.history).len(), 2, "all that was heard");
        assert_eq!(state.transcript.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1)], "what was perceived");

        let channel = Classification::general(Confidentiality::Confidential);
        state.turn = Some(TurnState { work: Work::new(Default::default(), Default::default(), channel.clone()), ..TurnState::default() });
        let confidential = Message { classification: channel.clone(), ..make_message(id(3)) };
        state.perceive(vec![confidential.clone()], vec![confidential]);
        assert_eq!(state.turn.as_ref().map(|t| t.work.label()), Some(channel), "the turn has seen it");
        assert_eq!(state.turn.as_ref().map(|t| t.work.context.transcript), Some(state.transcript.digest()), "and its next message follows it (issue 260)");
    }

    #[test]
    fn the_classifications_given_to_a_remote_agent_are_kept_across_its_calls() {
        use crate::message::tests::{id, make_message};
        let classified = |n, level| Message { classification: Classification::general(level), ..make_message(id(n)) };
        let mut state = AgentChannelState::default();
        state.transcript.extend(vec![classified(1, Confidentiality::Confidential), classified(2, Confidentiality::Internal)]);
        assert_eq!(state.classifications_given().len(), 2, "what it is given now");
        // The call is made: the transcript starts anew.
        state.transcript.start_anew();
        state.transcript.extend(vec![classified(3, Confidentiality::Internal)]);
        state.transcript.reclassify(id(3), Classification::PUBLIC);

        let given = state.classifications_given().to_vec();
        let expected = [Classification::general(Confidentiality::Confidential), Classification::general(Confidentiality::Internal), Classification::PUBLIC];
        assert_eq!(given, expected, "of the call before as well, each once, and of a change");
        assert_eq!(state.classifications_given().len(), 3, "asked again, nothing added");
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
        remote.open_tasks.push(OpenTask { task: TaskId("t-1".to_string()), since: Utc::now(), state: OpenTaskState::Working, unsaid: Some("so far".to_string()), work: None });
        assert_eq!(state.changes().len(), 6, "the small parts; the history and the transcript are the store's");
        replayed(&state, &mut copy);

        state.mark_stored();
        state.remote.open_tasks.clear();
        assert_eq!(state.changes().len(), 6, "the small parts alone");
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

    #[test]
    fn a_turn_begun_after_a_release_is_labelled_by_the_message_as_it_stands() {
        let mut state = a_state_that_heard("the numbers", Classification::CHANNEL_DEFAULT);
        let heard = messages_of(&state.history)[0].id;
        assert!(state.transcript.reclassify(heard, Classification::PUBLIC));

        begin(&mut state, Vec::new());
        assert_eq!(state.turn.as_ref().unwrap().work.label(), Classification::PUBLIC, "released, it no longer raises the label");

        let agent = ActorId::Agent(AgentId::new());
        let answer = Message { sender_id: agent, role: MessageRole::Assistant, turn_id: Some(TurnId::new()), ..said("hi", Classification::CHANNEL_DEFAULT) };
        state.transcript.push(answer.clone());
        state.transcript.reclassify(answer.id, Classification::PUBLIC);
        assert_eq!(state.retried(answer.id, agent), Ok(vec![answer.id]), "a change, not something said");
    }

    #[test]
    fn only_an_agents_last_answer_is_answered_again() {
        use crate::tool::{ToolCall, ToolCallResult, ToolResultStatus};
        let agent = ActorId::Agent(AgentId::new());
        let turn = TurnId::new();
        let answer = |content: &str| Message { sender_id: agent, role: MessageRole::Assistant, turn_id: Some(turn), ..said(content, Classification::CHANNEL_DEFAULT) };
        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        let question = messages_of(&state.history)[0].clone();
        let first = answer("hi");
        state.transcript.push(first.clone());
        state.history.push(first.clone(), None);

        assert_eq!(state.retried(first.id, agent), Ok(vec![first.id]), "its last answer");
        assert_eq!(state.retried(question.id, agent), Err(RetryRefused::NotItsAnswer), "not its own");
        assert_eq!(state.retried(first.id, ActorId::Agent(AgentId::new())), Err(RetryRefused::NotItsAnswer), "another agent's");

        // The user corrects the question: a change, not something said.
        state.transcript.modify(Message { content: Some("hello there".to_string()), ..question.clone() });
        assert_eq!(state.retried(first.id, agent), Ok(vec![first.id]), "still its last answer");

        // Its turn ended: taken over, it is still its last answer; given up,
        // what it said stands and nothing is taken back.
        let mut finished = state.clone();
        finished.transcript.record(TranscriptEntry::TurnFinished { turn_id: turn, at: Utc::now() });
        assert_eq!(finished.retried(first.id, agent), Ok(vec![first.id]), "its own turn's end");
        let mut given_up = state.clone();
        given_up.transcript.record(TranscriptEntry::TurnGivenUp { turn_id: turn, reason: GivenUpReason::Left, at: Utc::now() });
        assert_eq!(given_up.retried(first.id, agent), Err(RetryRefused::NotLast), "a turn given up");
        let mut compacted = state.clone();
        compacted.transcript.record(TranscriptEntry::Compacted { at: Utc::now() });
        assert_eq!(compacted.retried(first.id, agent), Err(RetryRefused::NotLast), "a compaction since, which may have summarised it (issue 387)");

        // What its turn did after it is taken back with it - not a step of
        // a later turn, nor a call that ran.
        let later_turn = Message { internal: true, turn_id: Some(TurnId::new()), ..answer("later") };
        let mut later = state.clone();
        later.transcript.push(later_turn);
        assert_eq!(later.retried(first.id, agent), Err(RetryRefused::NotLast), "a later turn followed");
        let call = ToolCall { id: Some("1".to_string()), function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let mut result = ToolCallResult::of(&call, ToolResultStatus::Ok, "done".to_string(), Classification::CHANNEL_DEFAULT);
        let step = Message { internal: true, ..answer("thinking") };
        state.transcript.push(step.clone());
        assert_eq!(state.retried(first.id, agent), Ok(vec![first.id, step.id]));
        result.started_at = Some(Utc::now());
        state.transcript.push(Message { internal: true, role: MessageRole::Tool, tool_results: vec![result], ..answer("") });
        assert_eq!(state.retried(first.id, agent), Err(RetryRefused::CallsRan));

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        state.transcript.push(first.clone());
        let later = said("and now?", Classification::CHANNEL_DEFAULT);
        state.perceive(vec![later.clone()], vec![later]);
        assert_eq!(state.retried(first.id, agent), Err(RetryRefused::NotLast), "something was said since");

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        state.transcript.push(first.clone());
        state.transcript.revoke(first.id, Utc::now());
        assert_eq!(state.retried(first.id, agent), Err(RetryRefused::NotItsAnswer), "taken back already");
        assert_eq!(serde_json::to_value(RetryRefused::NotLast).unwrap(), serde_json::json!("not_last"), "as a user interface reads it");
    }
}
