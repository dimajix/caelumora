//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see *Agent Cognitive Cycle* in `ARCHITECTURE.md`).
//!
//! A [`TurnContext`] is made for every step of a turn from the agent's state
//! in its channel, and handed to its components: it holds the turn's
//! messages, the context and prompt parts the model is given and where they
//! came from, pending tool calls and the plan, the changes of the turn not
//! yet taken over, and access to the agent's own state (issue 62) and to what
//! the session's other channels published (issue 17). `record_turn` and
//! `commit_agent_state` take the turn over when it completes.

use anyhow::{Context, Result};
use caelumora_model::agent::{AgentChannelState, AgentId, AgentStateChange};
use tracing::warn;
use std::mem;
use std::sync::Arc;

use crate::transport::EventSender;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::{ChannelClassification, Publication};
use crate::agent_state::AgentStateHandle;
use caelumora_model::digest::Digest;
use caelumora_model::agent::TurnKnowledge;
use caelumora_model::message::{ContextRef, KnowledgeRef, Message, MessageRole, PromptRef};
use caelumora_model::note::{Note, NoteChain, NoteChange, NoteId};
use crate::notes::SessionReader;
use caelumora_model::plan::{PlanProgress, PlanState};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::StateStore;
use caelumora_model::tool::ToolCallState;

/// An element of the context a `ContextProvider` hands a turn (issue 47):
/// the conversation, with the prompt parts placed where the provider wants
/// them. The provider decides where each part goes; the turn records it.
#[derive(Debug, Clone)]
pub enum ContextItem {
    /// An entry of the stored context, with the digest it has there.
    Conversation { message: Message, digest: Digest },
    /// A prompt part - the system prompt, notes, ... - that is not part of
    /// the conversation, given to the model in `role`.
    Prompt { role: MessageRole, text: String },
}

#[derive(Debug)]
pub struct TurnContext<'a> {
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub session_id: SessionId,
    pub agent_name: String,
    agent_channel_state: &'a mut AgentChannelState,
    session_reader: Option<Arc<dyn SessionReader>>,
    /// The agent's own state, shared by all its actors (issue 62). The turn
    /// reads it and stages changes, never holds it.
    agent_state: Arc<AgentStateHandle>,
}

impl<'a> TurnContext<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Self {
        Self {
            agent_id,
            channel_id: agent_channel_state.turn_event_context.channel_id,
            session_id: agent_channel_state.turn_event_context.session_id,
            agent_name,
            agent_channel_state,
            session_reader: None,
            agent_state,
        }
    }

    /// The agent's own state (issue 62): `current()` is a snapshot of it as
    /// it is now, without the changes of the current turn.
    pub fn agent_state(&self) -> &Arc<AgentStateHandle> {
        &self.agent_state
    }

    /// The agent's channel as the confidentiality rules see it, as it was
    /// when the turn began; `None` outside of a turn in a channel.
    pub fn classification(&self) -> Option<ChannelClassification> {
        self.agent_channel_state.turn_classification
    }

    /// Sets the agent's channel as it is when the turn begins.
    pub fn set_classification(&mut self, classification: Option<ChannelClassification>) {
        self.agent_channel_state.turn_classification = classification;
    }

    /// Stages `change` to the agent's own state, applied when the turn
    /// completes (`commit_agent_state`).
    pub fn stage_agent_change(&mut self, change: AgentStateChange) {
        self.agent_channel_state.turn_agent_changes.push(change);
    }

    /// The changes to the agent's own state staged in this turn so far.
    pub fn staged_agent_changes(&self) -> &[AgentStateChange] {
        &self.agent_channel_state.turn_agent_changes
    }

    /// Applies the changes the turn made to the agent's own state to that
    /// state as it is now (issue 62), anchored at the last message of the
    /// agent's history in this channel. Called when the turn completes,
    /// after `record_turn`.
    ///
    /// # Errors
    /// Whatever storing the state reports; the changes are then lost.
    pub async fn commit_agent_state(&mut self) -> Result<()> {
        let changes = mem::take(&mut self.agent_channel_state.turn_agent_changes);
        self.agent_state.apply(changes, self.agent_channel_state.history.last_message_id()).await
    }

    /// Gives the turn read access to what the session's agents and channels
    /// published (issue 17).
    pub fn with_session_reader(mut self, reader: Option<Arc<dyn SessionReader>>) -> Self {
        self.session_reader = reader;
        self
    }

    /// Read access to what the session's agents and channels published, if
    /// the turn has it.
    pub fn session_reader(&self) -> Option<Arc<dyn SessionReader>> {
        self.session_reader.clone()
    }

    pub fn next_turn(&mut self, event_context: EventContext, security_context: SecurityContext) {
        self.agent_channel_state.turn_scratch.clear();
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
        self.agent_channel_state.requested_state = None;
        self.agent_channel_state.plan = None;
        self.agent_channel_state.turn_prompts.clear();
        self.agent_channel_state.turn_notes.clear();
        self.agent_channel_state.turn_private_notes.clear();
        self.agent_channel_state.turn_knowledge.clear();
        self.agent_channel_state.turn_agent_changes.clear();
        self.agent_channel_state.turn_event_context = event_context;
        self.agent_channel_state.turn_security_context = security_context;
    }

    /// The event context of the current turn: carries the ids (`span_id`,
    /// `correlation_id`) that tie the turn to the envelope that started it.
    pub fn event_context(&self) -> &EventContext {
        &self.agent_channel_state.turn_event_context
    }

    pub fn finish_turn(&mut self) {
        self.agent_channel_state.turn_scratch.clear();
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
    }

    pub fn context(&self) -> &Vec<Message> {
        &self.agent_channel_state.turn_context
    }
    pub fn messages(&self) -> &Vec<Message> {
        &self.agent_channel_state.turn_messages
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.agent_channel_state.turn_messages.last()
    }

    pub fn full_context(&self) -> Vec<Message> {
        let mut context = self.agent_channel_state.turn_context.clone();
        context.extend_from_slice(self.agent_channel_state.turn_messages.as_slice());
        context
    }

    /// Sets the context of the turn: the messages the model is given, in
    /// order. Every prompt part is kept by digest and recorded with the
    /// entry it follows, so that what the agent saw can be told later
    /// (issues 06, 47); where it goes is the caller's decision.
    pub fn set_context(&mut self, items: Vec<ContextItem>) {
        let mut messages = Vec::with_capacity(items.len());
        let mut prompts = Vec::new();
        let mut after = None;
        for item in items {
            match item {
                ContextItem::Conversation { message, digest } => {
                    messages.push(message);
                    after = Some(digest);
                }
                ContextItem::Prompt { role, text } => {
                    let digest = caelumora_model::digest::prompt(&text);
                    self.agent_channel_state.prompts.entry(digest).or_insert_with(|| text.clone());
                    prompts.push(PromptRef { text: digest, role, after });
                    messages.push(Message {
                        role,
                        content: Some(text),
                        sender_id: ActorId::Agent(self.agent_id),
                        sender_name: self.agent_name.clone(),
                        ..Default::default()
                    });
                }
            }
        }
        self.agent_channel_state.turn_context = messages;
        self.agent_channel_state.turn_prompts = prompts;
    }

    pub fn push_message(&mut self, message: Message) {
        self.agent_channel_state.turn_messages.push(message);
    }

    pub fn set_tool_calls(&mut self, tool_calls: Vec<ToolCallState>) {
        self.agent_channel_state.tool_calls = tool_calls;
    }
    pub fn set_tool_approval_status(&mut self, id:ApprovalId, status: ApprovalStatus) -> bool {
        if let Some(tool_call) = self.agent_channel_state.tool_calls.iter_mut().find(|tc| tc.id == id) {
            tool_call.status = status;
            true
        }
        else {
            false
        }
    }
    pub fn tool_calls(&self) -> &Vec<ToolCallState> {
        &self.agent_channel_state.tool_calls
    }
    pub fn take_tool_calls(&mut self) -> Vec<ToolCallState> {
        mem::take(&mut self.agent_channel_state.tool_calls)
    }
    pub fn has_tool_calls(&self) -> bool {
        !self.agent_channel_state.tool_calls.is_empty()
    }

    pub fn set_plan(&mut self, plan: PlanState) {
        self.agent_channel_state.plan = Some(plan);
    }
    pub fn plan(&self) -> Option<&PlanState> {
        self.agent_channel_state.plan.as_ref()
    }
    pub fn take_plan(&mut self) -> Option<PlanState> {
        self.agent_channel_state.plan.take()
    }
    /// Whether a plan is currently in play. A `PlanState` is only ever
    /// constructed after its `create_plan` tool call has already been
    /// through the generic tool-call approval gate, so its mere presence
    /// implies "approved and executing".
    pub fn has_plan(&self) -> bool {
        self.agent_channel_state.plan.is_some()
    }
    pub fn set_plan_progress(&mut self, progress: PlanProgress) {
        if let Some(plan) = self.agent_channel_state.plan.as_mut() {
            plan.progress = progress;
        }
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.agent_channel_state.turn_security_context
    }

    pub fn state_context(&self) -> Vec<Message> {
        self.agent_channel_state.context.messages()
    }
    /// The stored context as items for [`Self::set_context`], each with its
    /// digest.
    pub fn stored_context(&self) -> Vec<ContextItem> {
        self.agent_channel_state.context.entries().iter()
            .map(|e| ContextItem::Conversation { message: e.message.clone(), digest: e.digest })
            .collect()
    }
    pub fn extend_state_context(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.context.extend(messages);
    }
    pub fn extend_state_history(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.history.extend(messages);
    }

    /// Records that the turn read `reference` (issue 17): it is recorded
    /// with every message the turn produces from now on.
    /// Whether the turn was given or read private knowledge of the agent -
    /// its private notes (issue 51) or its memory (issue 62) - so far: what the model produces from
    /// then on may reveal it (issue 61).
    pub fn saw_private_knowledge(&self) -> bool {
        self.agent_channel_state.turn_knowledge.iter()
            .any(|k| matches!(k.reference.source,
                caelumora_model::message::KnowledgeSource::PrivateNotes { .. } | caelumora_model::message::KnowledgeSource::Memory { .. }))
    }

    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        let before = self.agent_channel_state.turn_messages.len();
        let knowledge = &mut self.agent_channel_state.turn_knowledge;
        if !knowledge.iter().any(|k| k.reference == reference) {
            knowledge.push(TurnKnowledge { reference, before });
        }
    }

    /// The agent's notes in this channel as they stand - without the changes
    /// of the current turn, which count only once it completes.
    pub fn notes(&self) -> Vec<Note> {
        self.agent_channel_state.notes.current()
    }

    /// The digest of the agent's notes in this channel, `None` while it has
    /// none: the state of knowledge a reader of them records.
    pub fn notes_digest(&self) -> Option<caelumora_model::digest::Digest> {
        self.agent_channel_state.notes.digest()
    }

    /// Notes `text` in this turn and returns the note's number. It is
    /// published when the turn completes (issue 17).
    pub fn note(&mut self, text: String) -> NoteId {
        let state = &mut *self.agent_channel_state;
        add_note(&state.notes, &mut state.turn_notes, text)
    }

    /// Revokes the agent's note `id` in this turn: one of its current notes,
    /// or one noted earlier in this turn. `false` if there is no such note.
    pub fn revoke_note(&mut self, id: NoteId) -> bool {
        let state = &mut *self.agent_channel_state;
        revoke(&state.notes, &mut state.turn_notes, id)
    }

    /// The agent's private notes in this channel as they stand, without the
    /// changes of the current turn (issue 51).
    pub fn private_notes(&self) -> Vec<Note> {
        self.agent_channel_state.private_notes.current()
    }

    /// The digest of the agent's private notes in this channel.
    pub fn private_notes_digest(&self) -> Option<caelumora_model::digest::Digest> {
        self.agent_channel_state.private_notes.digest()
    }

    /// Notes `text` privately in this turn, like [`Self::note`]; numbered in
    /// the private notes, independently of the public ones.
    pub fn private_note(&mut self, text: String) -> NoteId {
        let state = &mut *self.agent_channel_state;
        add_note(&state.private_notes, &mut state.turn_private_notes, text)
    }

    /// Revokes one of the agent's private notes, like [`Self::revoke_note`].
    pub fn revoke_private_note(&mut self, id: NoteId) -> bool {
        let state = &mut *self.agent_channel_state;
        revoke(&state.private_notes, &mut state.turn_private_notes, id)
    }

    /// Appends the messages this agent produced in the turn to its context,
    /// and those that are not internal to its history - each with what it was
    /// produced from: the context before it and the turn's prompt parts
    /// (issues 06, 47).
    ///
    /// Takes over the notes changed in the turn, public and private, after
    /// its last message in the history, and returns whether public notes
    /// changed - which others are told of (issues 17, 51).
    pub fn record_turn(&mut self, messages: Vec<Message>) -> bool {
        let before = self.agent_channel_state.context.extend(messages.clone());
        let prompts = self.agent_channel_state.turn_prompts.clone();
        let knowledge = mem::take(&mut self.agent_channel_state.turn_knowledge);
        for (index, (message, context)) in messages.into_iter().zip(before).enumerate() {
            if !message.internal {
                // What was read before the message was produced.
                let knowledge = knowledge.iter().filter(|k| k.before <= index).map(|k| k.reference).collect();
                self.agent_channel_state.history.push(message, Some(ContextRef { context, prompts: prompts.clone(), knowledge }));
            }
        }

        // A message of the history, not an internal step of the context:
        // what branching (issue 08) will find the notes by.
        let after = self.agent_channel_state.history.last_message_id();
        let context = self.agent_channel_state.context.digest();
        let state = &mut *self.agent_channel_state;
        for change in mem::take(&mut state.turn_private_notes) {
            state.private_notes.push(change, after, context);
        }
        let changes = mem::take(&mut state.turn_notes);
        let changed = !changes.is_empty();
        for change in changes {
            state.notes.push(change, after, context);
        }
        changed
    }

    /// Records what state the turn should land on once `Reflecting` finishes,
    /// overriding the default `Idle`. This does not touch the live `ActorState`
    /// atomic directly — only the run loop does that; this is purely a same-turn
    /// handoff (survives across separate `TurnContext` borrows because it lives
    /// in `AgentChannelState`, not in `TurnContext` itself).
    pub fn request_state(&mut self, state: ActorState) {
        self.agent_channel_state.requested_state = Some(state);
    }

    /// Peek at a previously requested end-of-turn state without consuming it.
    pub fn requested_state(&self) -> Option<ActorState> {
        self.agent_channel_state.requested_state
    }

    /// Consume the requested end-of-turn state. Used once, by `Reflecting`.
    pub fn take_requested_state(&mut self) -> Option<ActorState> {
        self.agent_channel_state.requested_state.take()
    }

    pub async fn store_channel_state(&self, state_store: &Arc<dyn StateStore>) {
        if let Err(e) = state_store
            .put_agent_channel_state(self.agent_id, self.channel_id, self.agent_channel_state)
            .await
        {
            warn!(
                "Cannot store agent channel state of agent {}: {}", self.agent_id, e
            )
        }
    }

    /// Which labels the agent may publish to its channel in this turn.
    pub fn publication(&self) -> Publication {
        self.agent_channel_state.turn_publication
    }

    /// Sets which labels the agent may publish to its channel in this turn
    /// (issue 61), as the `SecurityManager` decided when it began.
    pub fn set_publication(&mut self, publication: Publication) {
        self.agent_channel_state.turn_publication = publication;
    }

    /// Sends `av` to the channel as the agent publishes it: messages redacted
    /// by the labels of their parts against what the turn may publish
    /// (issues 54, 61). The agent keeps the full messages in its context; an
    /// event with nothing left to show is not sent.
    pub async fn send_event(&self, av: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let publication = self.agent_channel_state.turn_publication;
        let Some(av) = av.published(&|label| publication.allows(label)) else { return Ok(()) };
        let ev = Envelope::new(
            self.agent_channel_state.turn_security_context.clone(),
            self.agent_channel_state.turn_event_context.clone(),
            av,
        );
        output
            .send(ev)
            .await
            .context(format!("Actor {} failed to send event", self.agent_id))?;
        Ok(())
    }
}

/// Adds a note with `text` to the changes `turn` of a turn on `chain`, and
/// returns its number: one more than any in the chain or the turn.
fn add_note(chain: &NoteChain, turn: &mut Vec<NoteChange>, text: String) -> NoteId {
    let written_now = turn.iter().filter_map(|c| match c {
        NoteChange::Note { id, .. } => Some(id.0),
        NoteChange::Revoke { .. } => None,
    }).max();
    let id = match written_now {
        Some(highest) => NoteId(highest + 1),
        None => chain.next_id(),
    };
    turn.push(NoteChange::Note { id, text });
    id
}

/// Revokes note `id` in the changes `turn` of a turn on `chain`: one of the
/// chain's current notes, or one noted earlier in the turn, which is then
/// simply dropped. `false` if there is no such note.
fn revoke(chain: &NoteChain, turn: &mut Vec<NoteChange>, id: NoteId) -> bool {
    if let Some(index) = turn.iter().position(|c| matches!(c, NoteChange::Note { id: noted, .. } if *noted == id)) {
        // Noted and revoked in the same turn: it was never published.
        turn.remove(index);
        return true;
    }
    let revoked_now = turn.iter().any(|c| matches!(c, NoteChange::Revoke { id: revoked } if *revoked == id));
    if revoked_now || !chain.current().iter().any(|n| n.id == id) {
        return false;
    }
    turn.push(NoteChange::Revoke { id });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::plan::{Plan, PlanId, PlanStep};

    fn make_plan_state() -> PlanState {
        PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: vec![PlanStep { description: "step 1".to_string() }],
            },
        }
    }

    #[test]
    fn plan_round_trips_through_set_and_take() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = TurnContext::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        assert!(ctx.plan().is_none());
        assert!(!ctx.has_plan());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());
        assert!(ctx.has_plan());

        let taken = ctx.take_plan();
        assert!(taken.is_some());
        assert!(ctx.plan().is_none());
        assert!(!ctx.has_plan());
    }

    #[test]
    fn next_turn_clears_a_stale_plan() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = TurnContext::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        ctx.next_turn(EventContext::default(), SecurityContext::default());
        assert!(ctx.plan().is_none());
    }

    #[test]
    fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        use caelumora_model::agent::{AgentChannelState, AgentId, SeenItem};
        use caelumora_model::message::{MessageId, MessageRole};

        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.context.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || ContextItem::Prompt { role: MessageRole::System, text: "You are good at sums.".to_string() };
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            let mut items = vec![system()];
            items.extend(turn.stored_context());
            turn.set_context(items);
            assert_eq!(turn.context().len(), 2, "the prompt is given to the model as a message");
            turn.record_turn(vec![first.clone(), second.clone()]);
        }

        let seen = state.seen_context_of(second.id).expect("the agent produced it");
        let order: Vec<String> = seen.items.iter().map(|item| match item {
            SeenItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
            SeenItem::Message(m) => m.content.clone().unwrap_or_default(),
        }).collect();
        assert_eq!(order, ["System:You are good at sums.", "What is 2+2?", "Let me think."], "everything before it, its own earlier message included");
        assert!(state.seen_context_of(question.id).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let (before, after) = (text(MessageRole::Assistant, "Let me look."), text(MessageRole::Assistant, "Found it."));
        let read = caelumora_model::message::KnowledgeRef {
            channel: Default::default(),
            source: caelumora_model::message::KnowledgeSource::History { digest: None },
        };
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.push_message(before.clone());
            turn.record_knowledge(read);
            turn.record_knowledge(read);
            turn.push_message(after.clone());
            let messages = turn.messages().clone();
            turn.record_turn(messages);
        }
        let knowledge_of = |id| state.recorded_context_of(id).unwrap().knowledge;
        assert!(knowledge_of(before.id).is_empty(), "produced before the read");
        assert_eq!(knowledge_of(after.id), vec![read], "produced after it, recorded once");

        // The same prompt in another turn is kept once.
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.set_context(vec![system()]);
        }
        assert_eq!(state.prompts.len(), 1);
    }

    #[tokio::test]
    async fn events_leave_the_agent_redacted_while_its_context_keeps_the_full_call() {
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::tool::{ToolCall, ToolConfidentiality};
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let output = crate::transport_impl::memory::InMemoryEventSender::new(tx);
        let mut state = AgentChannelState::default();
        let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
        turn.set_publication(Publication::new(true, true));

        let call = ToolCall { id: Some("1".to_string()), function_name: "note_private".to_string(), arguments: serde_json::json!({"text": "secret"}), confidentiality: ToolConfidentiality::HIDDEN };
        let message = Message { role: MessageRole::Assistant, tool_calls: vec![call], ..Message::default() };
        turn.push_message(message.clone());
        turn.send_event(ActorEvent::CompleteMessageGenerated(message.clone()), &output).await.unwrap();
        turn.send_event(ActorEvent::FullResponseGenerated(vec![message]), &output).await.unwrap();

        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::FullResponseGenerated(messages)) => assert!(messages.is_empty(), "the hidden call is not published"),
            other => panic!("the complete message is not sent at all, got {other:?}"),
        }
        assert_eq!(turn.messages()[0].tool_calls.len(), 1, "the agent keeps it");

        // Reasoning reaches the channel only as far as the turn may publish.
        let thinking = Message {
            role: MessageRole::Assistant,
            content: Some("done".to_string()),
            reasoning: Some("weighing".to_string()),
            reasoning_confidentiality: Confidentiality::Channel,
            ..Message::default()
        };
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking.clone()), &output).await.unwrap();
        turn.set_publication(Publication::new(true, false));
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking), &output).await.unwrap();
        let reasoning = |event: Option<Envelope>| match event.map(|e| e.event) {
            Some(ActorEvent::CompleteMessageGenerated(m)) => m.reasoning,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(reasoning(rx.recv().await).as_deref(), Some("weighing"), "a channel-level channel admits it");
        assert!(reasoning(rx.recv().await).is_none(), "a session-level one does not");
    }
}
