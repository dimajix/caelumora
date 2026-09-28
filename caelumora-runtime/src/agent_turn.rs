//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see *Agent Cognitive Cycle* in `ARCHITECTURE.md`).
//!
//! A [`Turn`] is made for every step of a turn from the agent's state
//! in its channel, and handed to its components: it holds the turn's
//! messages, the context and prompt parts the model is given and where they
//! came from, pending tool calls and the plan, the changes of the turn not
//! yet taken over, and generic access to the agent's state in the channel and
//! to its own state (issues 62, 65). What those states hold is the business
//! of wrappers such as `crate::agent_notes::AgentNotes` and
//! `crate::agent_memory::AgentMemory`. A turn is begun with `next_turn`, taken
//! over with `commit_turn` when it completes, and discarded with
//! `abandon_turn` when it fails.

use anyhow::{Context, Result};
use caelumora_model::agent::{AgentChannelChange, AgentChannelState, AgentId, AgentStateChange};
use std::mem;
use std::sync::Arc;
use tracing::warn;

use crate::transport::EventSender;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::{ChannelClassification, Publication};
use crate::agent_state::AgentStateHandle;
use caelumora_model::agent::TurnKnowledge;
use caelumora_model::message::{ContextItem, ContextRef, KnowledgeRef, Message, PromptRef};
use caelumora_model::plan::{PlanProgress, PlanState};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::tool::ToolCallState;

/// What is still needed of a turn once it is taken over (`commit_turn`).
#[derive(Debug)]
pub struct CommittedTurn {
    /// The messages that are not internal: the turn's answer, for
    /// `FullResponseGenerated`.
    pub final_messages: Vec<Message>,
    /// Whether the agent's state in the channel changed, to be announced
    /// once stored.
    pub channel_state_changed: bool,
}

#[derive(Debug)]
pub struct Turn<'a> {
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub session_id: SessionId,
    pub agent_name: String,
    agent_channel_state: &'a mut AgentChannelState,
    /// The agent's own state, shared by all its actors (issue 62). The turn
    /// reads it and stages changes, never holds it.
    agent_state: Arc<AgentStateHandle>,
}

impl<'a> Turn<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Self {
        Self {
            agent_id,
            channel_id: agent_channel_state.turn_event_context.channel_id,
            session_id: agent_channel_state.turn_event_context.session_id,
            agent_name,
            agent_channel_state,
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
    /// completes (`commit_turn`).
    pub fn stage_agent_change(&mut self, change: AgentStateChange) {
        self.agent_channel_state.turn_agent_changes.push(change);
    }

    /// The changes to the agent's own state staged in this turn so far.
    pub fn staged_agent_changes(&self) -> &[AgentStateChange] {
        &self.agent_channel_state.turn_agent_changes
    }

    /// Takes the completed turn over (issues 06, 17, 47, 62, 65):
    ///
    /// - appends the messages the agent produced (`push_message`) to its
    ///   transcript, and those that are not internal to its history, each
    ///   with what it was produced from - the transcript before it, the
    ///   turn's prompt parts and what it had read;
    /// - applies the changes staged to its state in this channel, anchored at
    ///   the turn's last message in the history;
    /// - applies the changes staged to its own state to that state as it is
    ///   now. The answer is already out, so a state that cannot be stored
    ///   costs the turn's memory, not the turn: it is logged;
    /// - clears the turn's working state (context, messages, plan, tool
    ///   calls), so that none of it is stored with the channel state.
    ///
    /// What is still needed afterwards is returned.
    pub async fn commit_turn(&mut self) -> CommittedTurn {
        let channel_state_changed = self.record_turn();
        let changes = mem::take(&mut self.agent_channel_state.turn_agent_changes);
        if let Err(e) = self.agent_state.apply(changes, self.agent_channel_state.history.last_message_id()).await {
            warn!("Agent {}: the changes of its turn to its own state are lost: {:#}", self.agent_id, e);
        }
        let final_messages = self.agent_channel_state.turn_messages.iter().filter(|m| !m.internal).cloned().collect();
        self.clear_work();
        CommittedTurn { final_messages, channel_state_changed }
    }

    /// Discards the turn after it failed: its working state, the changes it
    /// staged and what it read - none of it is taken over.
    pub fn abandon_turn(&mut self) {
        self.clear_work();
        self.agent_channel_state.turn_channel_changes.clear();
        self.agent_channel_state.turn_agent_changes.clear();
        self.agent_channel_state.turn_knowledge.clear();
    }

    /// Clears what the turn worked with.
    fn clear_work(&mut self) {
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
        self.agent_channel_state.plan = None;
        self.agent_channel_state.tool_calls.clear();
    }

    pub fn next_turn(&mut self, event_context: EventContext, security_context: SecurityContext) {
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
        self.agent_channel_state.requested_state = None;
        self.agent_channel_state.plan = None;
        self.agent_channel_state.turn_channel_changes.clear();
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

    /// The context of the turn as the model is given it: the conversation,
    /// with every prompt part as a message of the agent in its role.
    pub fn context(&self) -> Vec<Message> {
        self.agent_channel_state.turn_context.iter()
            .map(|item| match item {
                ContextItem::Conversation { message, .. } => message.clone(),
                ContextItem::Prompt { role, text } => Message {
                    role: *role,
                    content: Some(text.clone()),
                    sender_id: ActorId::Agent(self.agent_id),
                    sender_name: self.agent_name.clone(),
                    ..Default::default()
                },
            })
            .collect()
    }

    /// Where the turn's prompt parts were placed: each by the digest of its
    /// text, with the transcript entry it followed (issue 47).
    fn prompt_refs(&self) -> Vec<PromptRef> {
        let mut after = None;
        let mut prompts = Vec::new();
        for item in &self.agent_channel_state.turn_context {
            match item {
                ContextItem::Conversation { digest, .. } => after = Some(*digest),
                ContextItem::Prompt { role, text } => {
                    prompts.push(PromptRef { text: caelumora_model::digest::prompt(text), role: *role, after });
                }
            }
        }
        prompts
    }
    pub fn messages(&self) -> &Vec<Message> {
        &self.agent_channel_state.turn_messages
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.agent_channel_state.turn_messages.last()
    }

    pub fn full_context(&self) -> Vec<Message> {
        let mut context = self.context();
        context.extend_from_slice(self.agent_channel_state.turn_messages.as_slice());
        context
    }

    /// Sets the context of the turn: the conversation with the prompt parts
    /// where the caller placed them. Every prompt part's text is kept once by
    /// digest, so that what the agent saw can be told later (issues 06, 47).
    pub fn set_context(&mut self, items: Vec<ContextItem>) {
        for item in &items {
            if let ContextItem::Prompt { text, .. } = item {
                let digest = caelumora_model::digest::prompt(text);
                self.agent_channel_state.prompts.entry(digest).or_insert_with(|| text.clone());
            }
        }
        self.agent_channel_state.turn_context = items;
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

    pub fn transcript(&self) -> Vec<Message> {
        self.agent_channel_state.transcript.messages()
    }
    pub fn extend_transcript(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.transcript.extend(messages);
    }
    pub fn extend_history(&mut self, messages: Vec<Message>) {
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

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes the turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &AgentChannelState {
        self.agent_channel_state
    }

    /// Stages `change` to the agent's state in this channel, applied when the
    /// turn completes (`commit_turn`).
    pub fn stage_channel_change(&mut self, change: AgentChannelChange) {
        self.agent_channel_state.turn_channel_changes.push(change);
    }

    /// The changes to the agent's state in this channel staged in this turn
    /// so far - to change as well: a change taken back within the turn is
    /// simply dropped.
    pub fn staged_channel_changes(&mut self) -> &mut Vec<AgentChannelChange> {
        &mut self.agent_channel_state.turn_channel_changes
    }

    /// The part of `commit_turn` that concerns the agent's state in this
    /// channel; returns whether changes were staged to it.
    fn record_turn(&mut self) -> bool {
        let messages = self.agent_channel_state.turn_messages.clone();
        let before = self.agent_channel_state.transcript.extend(messages.clone());
        let prompts = self.prompt_refs();
        let knowledge = mem::take(&mut self.agent_channel_state.turn_knowledge);
        for (index, (message, transcript)) in messages.into_iter().zip(before).enumerate() {
            if !message.internal {
                // What was read before the message was produced.
                let knowledge = knowledge.iter().filter(|k| k.before <= index).map(|k| k.reference).collect();
                self.agent_channel_state.history.push(message, Some(ContextRef { transcript, prompts: prompts.clone(), knowledge }));
            }
        }

        // A message of the history, not an internal step of the transcript:
        // what branching (issue 08) will find the changes by.
        let after = self.agent_channel_state.history.last_message_id();
        let transcript = self.agent_channel_state.transcript.digest();
        let changes = mem::take(&mut self.agent_channel_state.turn_channel_changes);
        let changed = !changes.is_empty();
        for change in changes {
            self.agent_channel_state.apply(change, after, transcript);
        }
        changed
    }

    /// Records what state the turn should land on once `Reflecting` finishes,
    /// overriding the default `Idle`. This does not touch the live `ActorState`
    /// atomic directly — only the run loop does that; this is purely a same-turn
    /// handoff (survives across separate `Turn` borrows because it lives
    /// in `AgentChannelState`, not in `Turn` itself).
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageRole;
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
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

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
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        ctx.next_turn(EventContext::default(), SecurityContext::default());
        assert!(ctx.plan().is_none());
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        use caelumora_model::agent::{AgentChannelState, AgentId, SeenItem};
        use caelumora_model::message::{MessageId, MessageRole};

        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || ContextItem::Prompt { role: MessageRole::System, text: "You are good at sums.".to_string() };
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            let mut items = vec![system()];
            items.extend(turn.agent_channel_state().transcript.entries().iter()
                .map(|e| ContextItem::Conversation { message: e.message.clone(), digest: e.digest }));
            turn.set_context(items);
            assert_eq!(turn.context().len(), 2, "the prompt is given to the model as a message");
            turn.push_message(first.clone());
            turn.push_message(second.clone());
            turn.commit_turn().await;
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
            source: caelumora_model::message::KnowledgeSource::History { digest: None, start: None },
        };
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.push_message(before.clone());
            turn.record_knowledge(read);
            turn.record_knowledge(read);
            turn.push_message(after.clone());
            turn.commit_turn().await;
        }
        let knowledge_of = |id| state.recorded_context_of(id).unwrap().knowledge;
        assert!(knowledge_of(before.id).is_empty(), "produced before the read");
        assert_eq!(knowledge_of(after.id), vec![read], "produced after it, recorded once");

        // The same prompt in another turn is kept once.
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
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
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
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
