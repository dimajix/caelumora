use crate::actor::{ActorId, ActorState, EventContext};
use crate::component::ComponentConfig;
use crate::digest::Digest;
use crate::message::{ContextItem, ContextRef, Message, Transcript, MessageHistory, MessageId, PromptRef};
use crate::note::{NoteChain, NoteChange};
use crate::meta::{Metadata, Scope};
use crate::plan::PlanState;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::tool::ToolCallState;
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
    /// Whether the agent first analyzes the request and proposes a
    /// structured plan requiring user approval before executing it, instead
    /// of answering directly, is derived from whether `components` wires in
    /// the `create_plan_tool` component - not a separate flag here, since
    /// extended planning is just another tool a personality opts into.
    #[serde(default)]
    pub components: ComponentConfig,
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
    /// The least confidential label the reasoning of its agents gets
    /// (issue 61); what a turn saw may raise it. `public` unless named.
    #[serde(default = "public_reasoning")]
    pub reasoning_confidentiality: crate::confidentiality::Confidentiality,
}

fn public_reasoning() -> crate::confidentiality::Confidentiality {
    crate::confidentiality::Confidentiality::Public
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
    /// that from the owner, in the channel it runs in (issue 87).
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
                description: personality.metadata.description.clone(),
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
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
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
    /// The turn under way, `None` between turns (issue 85). Stored with the
    /// rest, so that a turn cut off by a crash is resumed (issue 75).
    pub turn: Option<TurnState>,
}

/// The working state of one turn (issue 85): what the turn is - fixed when
/// it begins - and what it has produced and staged so far, none of it taken
/// over yet. Taking the turn over (`commit_turn`) or discarding it
/// (`abandon_turn`) ends it as a whole; a branch switch (issue 08) will
/// discard it the same way.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// The envelope the turn answers: carries the ids (`span_id`,
    /// `correlation_id`) that tie the turn to what started it.
    pub event_context: EventContext,
    /// Whose authority the turn acts with.
    pub security_context: SecurityContext,
    /// The agent's channel as the confidentiality rules see it, as it was
    /// when the turn began (issue 61): the turn is consistent in itself
    /// should the channel's level change (issue 67).
    pub classification: crate::confidentiality::ChannelClassification,
    /// Which labels the agent may publish to the channel in this turn, as
    /// the `SecurityManager` decided when it began (issue 61).
    pub publication: crate::confidentiality::Publication,
    /// The context as its `ContextProvider` gave it: what the model is sent
    /// and where the prompt parts were placed are both derived from it
    /// (issue 47).
    pub context: Vec<crate::message::ContextItem>,
    /// The messages the turn produced so far.
    pub messages: Vec<Message>,
    /// Knowledge read in the turn, each with the number of messages the
    /// turn had produced by then: it counts for the messages after it
    /// (issue 17).
    pub knowledge: Vec<TurnKnowledge>,
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
}

/// Knowledge read in a turn, and how many messages the turn had produced
/// before it was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnKnowledge {
    pub reference: crate::message::KnowledgeRef,
    pub before: usize,
}

/// What an agent's model saw when it produced a message (issues 06, 47):
/// its context - the messages of its transcript and the prompt parts of that
/// turn, in the order the model was given them, as the items it was given
/// (issue 119).
#[derive(Debug, Clone)]
pub struct SeenContext {
    /// The digests the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<crate::message::ContextItem>,
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
            AgentChannelChange::Note(change) => self.notes.push(change, after, transcript),
            AgentChannelChange::PrivateNote(change) => self.private_notes.push(change, after, transcript),
        }
    }

    /// Resolves `reference` in this state: the context prefix it names, with
    /// every prompt part inserted after the entry it followed. `None` if what
    /// it names is not here - a context that is not a prefix of this one, a
    /// prompt that was never recorded, or one whose entry is not in the
    /// prefix.
    pub fn resolve(&self, reference: &ContextRef) -> Option<SeenContext> {
        let entries = match &reference.transcript {
            Some(digest) => self.transcript.prefix_entries(digest)?,
            None => &[],
        };
        let prompt = |p: &PromptRef| -> Option<ContextItem> {
            Some(ContextItem::Prompt { digest: p.text, role: p.role, text: self.prompts.get(&p.text)?.clone() })
        };
        let mut items = Vec::with_capacity(entries.len() + reference.prompts.len());
        for p in reference.prompts.iter().filter(|p| p.after.is_none()) {
            items.push(prompt(p)?);
        }
        for entry in entries {
            items.push(ContextItem::Conversation { message: entry.message.clone(), digest: entry.digest });
            for p in reference.prompts.iter().filter(|p| p.after == Some(entry.digest)) {
                items.push(prompt(p)?);
            }
        }
        if items.len() != entries.len() + reference.prompts.len() {
            return None;
        }
        Some(SeenContext { reference: reference.clone(), items })
    }

    /// What the history recorded the message `id` was produced from, or
    /// `None` if this agent did not produce it (or it was before references
    /// were recorded).
    pub fn recorded_context_of(&self, id: MessageId) -> Option<ContextRef> {
        self.history.entries().iter().find(|e| e.message.id == id)?.context.clone()
    }

    /// What the agent saw when it produced the message `id` of its history,
    /// or `None` if it did not produce it or it cannot be resolved.
    pub fn seen_context_of(&self, id: MessageId) -> Option<SeenContext> {
        self.resolve(&self.recorded_context_of(id)?)
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
    /// A change to the agent's memory, made in the channel `origin`.
    Memory { change: crate::memory::MemoryChange, origin: crate::confidentiality::ChannelClassification },
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

    #[test]
    fn a_turn_under_way_is_stored_with_the_state_and_none_between_turns() {
        let idle = AgentChannelState::default();
        let json = serde_json::to_string(&idle).unwrap();
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(decoded.turn.is_none() && decoded.processing_state == ActorState::Idle);

        let waiting = AgentChannelState {
            processing_state: ActorState::WaitingForApproval,
            turn: Some(TurnState { messages: vec![Message::default()], ..TurnState::default() }),
            ..AgentChannelState::default()
        };
        let json = serde_json::to_string(&waiting).unwrap();
        assert!(json.contains("\"WaitingForApproval\""));
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.processing_state, ActorState::WaitingForApproval);
        assert_eq!(decoded.turn.map(|t| t.messages.len()), Some(1), "the turn is resumed from what was stored");
    }

    #[test]
    fn prompt_parts_are_resolved_where_they_were_placed() {
        use crate::digest::prompt;
        use crate::message::MessageRole;
        let text = |content: &str| Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let (earlier, question) = (text("earlier"), text("question"));
        let after_earlier = state.transcript.push(earlier.clone());
        assert_eq!(after_earlier, None);
        let before_question = state.transcript.push(question.clone());
        let whole = state.transcript.digest();
        for part in ["system", "notes"] {
            state.prompts.insert(prompt(part), part.to_string());
        }
        let reference = ContextRef {
            transcript: whole,
            prompts: vec![
                PromptRef { text: prompt("system"), role: MessageRole::System, after: None },
                PromptRef { text: prompt("notes"), role: MessageRole::User, after: before_question },
            ],
            knowledge: Vec::new(),
        };

        let seen = state.resolve(&reference).expect("everything it names is here");
        let order: Vec<String> = seen.items.iter().map(|item| match item {
            ContextItem::Prompt { text, .. } => format!("prompt:{text}"),
            ContextItem::Conversation { message, .. } => message.content.clone().unwrap_or_default(),
        }).collect();
        assert_eq!(order, ["prompt:system", "earlier", "prompt:notes", "question"]);

        let mut elsewhere = reference.clone();
        elsewhere.prompts[1].after = Some(prompt("not an entry"));
        assert!(state.resolve(&elsewhere).is_none(), "a part after an entry that is not in the prefix");
        let mut forgotten = reference;
        forgotten.prompts[0].text = prompt("never recorded");
        assert!(state.resolve(&forgotten).is_none(), "a part whose text was not kept");
    }

    #[test]
    fn a_channel_change_goes_to_the_chain_it_names() {
        use crate::note::{NoteChange, NoteId};
        let mut state = AgentChannelState::default();
        let note = |id, text: &str| NoteChange::Note { id: NoteId(id), text: text.to_string() };
        state.apply(AgentChannelChange::Note(note(1, "public")), MessageId::new(), None);
        state.apply(AgentChannelChange::PrivateNote(note(1, "private")), MessageId::new(), None);
        assert_eq!(state.notes.current()[0].text, "public");
        assert_eq!(state.private_notes.current()[0].text, "private");
    }
}
