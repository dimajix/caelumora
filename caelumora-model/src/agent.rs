use crate::actor::{ActorId, ActorState, EventContext, ProcessingStateHolder};
use crate::component::ComponentConfig;
use crate::event::{EventPublisher, GlobalEvent};
use crate::digest::Digest;
use crate::message::{ContextRef, Message, Transcript, MessageHistory, MessageId, PromptRef};
use crate::note::{NoteChain, NoteChange};
use crate::meta::{Metadata, Scope};
use crate::plan::PlanState;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::session::SessionId;
use crate::store::{EntityStore, StoreResult};
use crate::tool::ToolCallState;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct AgentController {
    id: AgentId,
    session_id: SessionId,
    agent: ArcSwap<Agent>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
}

impl AgentController {
    pub fn new(
        session_id: SessionId,
        agent: Agent,
        event_publisher: Arc<dyn EventPublisher>,
        entity_store: Arc<dyn EntityStore>
    ) -> Self {
        Self {
            id: agent.id,
            session_id,
            agent: ArcSwap::from_pointee(agent),
            event_publisher,
            entity_store,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Agent>> {
        self.agent.load()
    }

    pub fn id(&self) -> AgentId {
        self.id
    }

    pub fn name(&self) -> String {
        self.agent.load().metadata.name.clone()
    }

    /// Renames the agent on behalf of `ctx`: stored first - which is where
    /// the permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// Whatever the store reports; the agent is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: String) -> StoreResult<()> {
        let event = AgentUpdateEvent::NameChanged(new_name);
        let agent = self.apply(&event);
        self.entity_store.update_agent(ctx, &agent).await?;
        let attributes = agent.security_attributes.clone();
        self.agent.store(Arc::new(agent));
        self.event_publisher.publish(GlobalEvent::AgentUpdated(self.session_id, self.id, event), attributes);
        Ok(())
    }

    /// The agent with `event` applied, not yet stored anywhere.
    fn apply(&self, event: &AgentUpdateEvent) -> Agent {
        let mut agent = (**self.agent.load()).clone();
        match event {
            AgentUpdateEvent::NameChanged(name) => agent.metadata.name = name.clone(),
        }
        agent
    }

    /// Takes over an update made elsewhere - already stored there.
    fn process_update(&self, event: &AgentUpdateEvent) {
        self.agent.store(Arc::new(self.apply(event)));
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::AgentUpdated(_, aid, update) if *aid == self.id => {
                self.process_update(update);
            }
            _ => {}
        }
    }
}

define_id!(PersonalityId);

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
    pub security_context: SecurityContext,
}

impl Agent {
    pub fn new(name:String, scope:Scope, personality: &Personality, mut security_context: SecurityContext) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        security_context.caller_id = ActorId::Agent(agent_id);
        // Owned by whoever created it; the agent itself contributes, so that
        // it may read its own definition (issue 11).
        let security_attributes = SecurityAttributes {
            owner_id: security_context.initiator_id,
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
            security_context,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentChannelState {
    #[serde(
        default = "crate::actor::default_processing_state",
        serialize_with = "crate::actor::processing_state_serde::serialize",
        deserialize_with = "crate::actor::processing_state_serde::deserialize"
    )]
    pub processing_state: Arc<ProcessingStateHolder>,
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
    pub transcript: Transcript,
    /// The notes the agent published in this channel (issue 17).
    #[serde(default)]
    pub notes: NoteChain,
    /// The agent's private notes in this channel, which only it reads - and
    /// only where the channel's confidentiality allows (issue 51).
    #[serde(default)]
    pub private_notes: NoteChain,
    /// Knowledge read in the current turn, each with the number of messages
    /// the turn had produced by then: it counts for the messages after it
    /// (issue 17).
    #[serde(default)]
    pub turn_knowledge: Vec<TurnKnowledge>,
    /// Which labels the agent may publish to the channel in the current turn
    /// (issue 61), decided when the turn began. Not stored: a turn resumed
    /// after a restart publishes only what is public until its next turn.
    #[serde(skip)]
    pub turn_publication: crate::confidentiality::Publication,
    /// The agent's channel as the confidentiality rules see it, as it was
    /// when the current turn began (issue 61). Not stored, like
    /// `turn_publication`.
    #[serde(skip)]
    pub turn_classification: Option<crate::confidentiality::ChannelClassification>,
    /// Changes to the agent's `AgentState` made in the current turn, applied
    /// to the current state when the turn completes (issue 62).
    #[serde(default)]
    pub turn_agent_changes: Vec<AgentStateChange>,
    /// Changes to this state made in the current turn - to its notes and
    /// private notes - applied when the turn completes and discarded if it
    /// does not (issue 65).
    #[serde(default)]
    pub turn_channel_changes: Vec<AgentChannelChange>,
    /// Every prompt part the agent was given in this channel - the system
    /// prompt, notes, ... - by digest, each kept once: what the `ContextRef`s
    /// in the history point to (issues 06, 47).
    #[serde(default)]
    pub prompts: BTreeMap<Digest, String>,
    /// Pending tool calls
    #[serde(default)]
    pub tool_calls: Vec<ToolCallState>,
    /// The plan currently in progress for this turn, if any.
    #[serde(default)]
    pub plan: Option<PlanState>,
    /// Set by a tool (e.g. stop/bye/silence) or by the run loop itself (on a
    /// detected cancellation) to override what state `Reflecting` lands on
    /// instead of the default `Idle`. Consumed once, at the end of the turn.
    #[serde(default)]
    pub requested_state: Option<ActorState>,
    /// The context of the current turn as its `ContextProvider` gave it:
    /// what the model is sent and where the prompt parts were placed are
    /// both derived from it (issue 47). Stored, so that a turn waiting for
    /// an approval can be recorded after a restart.
    #[serde(default)]
    pub turn_context: Vec<crate::message::ContextItem>,
    #[serde(default)]
    pub turn_messages: Vec<Message>,
    #[serde(default)]
    pub turn_security_context: SecurityContext,
    #[serde(default)]
    pub turn_event_context: EventContext
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
/// turn, in the order the model was given them.
#[derive(Debug, Clone)]
pub struct SeenContext {
    /// The digests the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<SeenItem>,
}

/// One element of a [`SeenContext`].
#[derive(Debug, Clone)]
pub enum SeenItem {
    Prompt { digest: Digest, role: crate::message::MessageRole, text: String },
    Message(Message),
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
        let prompt = |p: &PromptRef| -> Option<SeenItem> {
            Some(SeenItem::Prompt { digest: p.text, role: p.role, text: self.prompts.get(&p.text)?.clone() })
        };
        let mut items = Vec::with_capacity(entries.len() + reference.prompts.len());
        for p in reference.prompts.iter().filter(|p| p.after.is_none()) {
            items.push(prompt(p)?);
        }
        for entry in entries {
            items.push(SeenItem::Message(entry.message.clone()));
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
    #[serde(default)]
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
    fn test_agent_channel_state_serialization() {
        let state = AgentChannelState::default();
        state.processing_state.store(ActorState::WaitingForApproval.into());
        
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"WaitingForApproval\""));
        
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(ActorState::from(decoded.processing_state.load()), ActorState::WaitingForApproval);
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
            SeenItem::Prompt { text, .. } => format!("prompt:{text}"),
            SeenItem::Message(m) => m.content.clone().unwrap_or_default(),
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
