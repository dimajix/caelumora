//! How a turn ends: taken over when it completes (`Turn::commit`), or
//! given up when it fails or is stopped (`Turn::give_up`), which keeps
//! its messages as a branch of the transcript that is no longer active
//! (issue 145). Both end it and return it as an [`EndedTurn`], which sends
//! the turn's last event.

use super::{send, Turn};
use crate::tool_calls::ToolCalls;
use crate::transport::EventSender;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, EventContext};
use caelumora_model::turn::TurnState;
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::{executed_calls, Message};
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolCallState};
use caelumora_model::transcript::GivenUpReason;
use tracing::warn;

/// What is left of a turn once it ended - taken over or given up: what its
/// last events are sent with, the contexts the turn had (issue 85), and the
/// tool calls it abandoned (issue 148). Nothing else; the rest of the turn
/// was used up in ending it.
#[derive(Debug)]
pub struct EndedTurn {
    event_context: EventContext,
    security_context: SecurityContext,
    abandoned_calls: Vec<ToolCallState>,
}

impl EndedTurn {
    fn of(turn: &TurnState, abandoned_calls: Vec<ToolCallState>) -> Self {
        Self {
            event_context: turn.event_context.clone(),
            security_context: turn.security_context.clone(),
            abandoned_calls,
        }
    }

    /// The tool calls that had no result when the turn ended, answered as
    /// not executed (`Turn::abandon_tool_calls`, issue 148).
    pub(crate) fn abandoned_calls(&self) -> &[ToolCallState] {
        &self.abandoned_calls
    }

    /// Sends `event` as the ended turn would have: see [`Turn::send_event`].
    pub(crate) async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        send(&self.event_context, &self.security_context, event, output).await
    }
}

/// What is still needed of a turn once it is taken over (`commit`).
#[derive(Debug)]
pub struct CommittedTurn {
    /// The messages that are not internal: the turn's answer, for
    /// `FullResponseGenerated`.
    pub final_messages: Vec<Message>,
    /// Whether the agent's state in the channel changed, to be announced
    /// once stored.
    pub channel_state_changed: bool,
    /// The turn itself, to send its answer with.
    pub ended: EndedTurn,
}

/// Why a turn was given up, as the agent is told: "… because it failed".
fn because(reason: &GivenUpReason) -> &'static str {
    match reason {
        GivenUpReason::Failed { .. } => "it failed",
        GivenUpReason::Lowered { .. } => "the channel's classification was lowered",
        GivenUpReason::Stopped => "you were stopped",
    }
}

/// What the agent is told of the calls that ran in a turn given up
/// (`executed`), and why it was given up: with arguments and results
/// `in_full`, else only which tools ran - the turn does no longer flow into
/// the channel, whose classification was lowered meanwhile (issue 141), so
/// what it saw may not either.
fn told_of_executed(reason: &GivenUpReason, executed: &[(Option<&ToolCall>, &ToolCallResult)], in_full: bool) -> String {
    let because = because(reason);
    let calls: Vec<String> = executed.iter().map(|(call, result)| match in_full {
        true => {
            let arguments = call.map(|c| c.arguments.to_string()).unwrap_or_default();
            let status = serde_json::to_string(&result.status).unwrap_or_default();
            format!("- {}({}) -> {}: {}", result.tool_name, arguments, status.trim_matches('"'), result.content)
        }
        false => format!("- {}", result.tool_name),
    }).collect();
    let (what, hidden) = match in_full {
        true => ("these tool calls", ""),
        false => ("calls of these tools", "Their arguments and results cannot be shown here: the channel's classification was lowered since. "),
    };
    format!(
        "Your previous turn was given up before it was complete, because {because}. Its messages are gone, but {what} had already been executed, and their effects remain:\n{}\n{hidden}Do not repeat them unless that is what is wanted.",
        calls.join("\n"))
}

impl Turn<'_> {
    /// Takes the completed turn over and ends it (issues 06, 17, 47, 62, 65):
    ///
    /// - appends the messages the agent produced (`push_message`) to its
    ///   transcript, and those that are not internal to its history, each
    ///   with what it was produced from (issue 136) - the transcript before
    ///   it, the prompt parts placed before it and what it had read;
    /// - applies the changes staged to its state in this channel, anchored at
    ///   the turn's last message in the history;
    /// - applies the changes staged to its own state to that state as it is
    ///   now. The answer is already out, so a state that cannot be stored
    ///   costs the turn's memory, not the turn: it is logged.
    ///
    /// A step whose calls do not all have their result yet - only a
    /// `cancelled` turn is taken over so - are abandoned first
    /// (`abandon_tool_calls`).
    ///
    /// The turn's working state is gone afterwards, so none of it is stored
    /// with the channel state. What is still needed is returned.
    pub(crate) async fn commit(mut self, cancelled: bool) -> CommittedTurn {
        let abandoned_calls = self.abandon_tool_calls(if cancelled { "it was cancelled" } else { "it ended" });
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let ended = EndedTurn::of(&turn, abandoned_calls);
        // Every part is used up exactly once.
        let final_messages = turn.messages().filter(|m| !m.internal).cloned().collect();
        let TurnState { entries, memory, notes, private_notes, .. } = turn;
        let channel_state_changed = self.agent_channel_state.record_turn(entries, notes, private_notes);
        if let Err(e) = self.agent_state.apply(memory, self.agent_channel_state.history.last_message_id()).await {
            warn!("Agent {}: the changes of its turn to its own state are lost: {:#}", self.agent_id, e);
        }
        CommittedTurn { final_messages, channel_state_changed, ended }
    }

    /// Gives the turn up after it failed or was stopped, for `reason`
    /// (issue 145): its messages go into the transcript and the history as
    /// they would when taken over, each with what it was produced from, but
    /// as a branch that is no longer active - both heads go back to where
    /// the turn began, and the transcript records the turn as given up,
    /// naming the branch. Nothing it staged is applied.
    ///
    /// What its tools did stays done. If any of its calls ran, the agent is
    /// told on the active branch, in a message of the runtime in its
    /// transcript: in full, classified as the turn's label (issue 168) - or,
    /// with `names_only`, only which tools ran, classified so. A step whose
    /// calls do not all have their result yet is ended first
    /// (`abandon_tool_calls`), so that those that ran are told too.
    pub(crate) fn give_up(mut self, reason: GivenUpReason, names_only: Option<Classification>) -> EndedTurn {
        let abandoned_calls = self.abandon_tool_calls(because(&reason));
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let ended = EndedTurn::of(&turn, abandoned_calls);
        let label = turn.label();
        let TurnState { entries, .. } = turn;
        let messages: Vec<&Message> = entries.iter().filter_map(|e| e.node.entry.message()).collect();
        let executed = executed_calls(&messages);
        let told = (!executed.is_empty()).then(|| Message {
            id: caelumora_model::message::MessageId::new(),
            content: Some(told_of_executed(&reason, &executed, names_only.is_none())),
            internal: true,
            ..Message::new(names_only.unwrap_or(label))
        });
        self.agent_channel_state.give_up_turn(entries, reason);

        if let Some(message) = told {
            self.agent_channel_state.transcript.push(message);
        }
        ended
    }

    /// Abandons the tool calls of the current step as the turn ends,
    /// `because` - why it ends (issue 148): those without a result are
    /// answered as not executed, and the tool message is kept like any
    /// other, internal like the step. No call of the turn is left unanswered
    /// in the transcript, and the results of those that ran are not lost
    /// with the turn's working state. Returns the calls abandoned.
    fn abandon_tool_calls(&mut self, because: &str) -> Vec<ToolCallState> {
        let internal = self.in_plan_step();
        match self.agent_channel_state.turn.as_mut() {
            Some(turn) => ToolCalls::new(turn, &self.agent_name).abandon(because, internal),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_notes::NoteKind;
    use crate::agent_state::AgentStateHandle;
    use crate::agent_turn::tests::message;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::message::MessageId;
    use caelumora_model::message::MessageRole;

    /// A turn begun on `state`, whose model called `tool` with `arguments`;
    /// the call ran if `ran`, and a note was staged.
    fn a_turn_that_called<'a>(state: &'a mut AgentChannelState, tool: &str, ran: bool) -> Turn<'a> {
        use caelumora_model::note::{NoteChange, NoteId};
        use caelumora_model::tool::{ToolCall, ToolCallResult, ToolResultStatus, ToolVisibility};
        let mut turn = Turn::begun(AgentId::default(), String::new(), state, crate::agent_state::AgentStateHandle::detached());
        let call = ToolCall { id: Some("1".to_string()), function_name: tool.to_string(), arguments: serde_json::json!({ "to": "bob" }), visibility: ToolVisibility::SHOWN };
        let mut result = ToolCallResult::of(&call, ToolResultStatus::Ok, "sent".to_string(), Classification::CHANNEL_DEFAULT);
        result.started_at = ran.then(chrono::Utc::now);
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Assistant, tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Tool, tool_results: vec![result], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.stage_note(NoteKind::Public, NoteChange::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT });
        turn
    }

    fn a_state_with(content: &str) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let message = Message { id: MessageId::new(), role: MessageRole::User, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.transcript.push(message.clone());
        state.history.push(message, None);
        state
    }

    #[test]
    fn a_turn_given_up_stays_as_a_branch_and_nothing_it_staged_is_applied() {
        let mut state = a_state_with("mail bob");
        let (transcript, history) = (state.transcript.digest(), state.history.digest());
        let turn = a_turn_that_called(&mut state, "send_mail", false);
        turn.give_up(GivenUpReason::Stopped, None);

        assert!(state.turn.is_none(), "the turn is over");
        assert_eq!(state.history.digest(), history, "the history is where the turn began");
        assert_ne!(state.transcript.digest(), transcript, "the transcript records the turn given up");
        assert_eq!(state.transcript.entries().len(), 2, "after what was there: nothing ran, so nothing is told");
        assert_eq!(state.transcript.messages().len(), 1, "and the model is given what was there");
        let turns = state.transcript.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert_eq!((turns[0].given_up.reason.clone(), turns[0].messages.len()), (GivenUpReason::Stopped, 2));
        assert_eq!(state.history.all_entries().len(), 3, "its messages are in the history too, on their branch");
        assert!(state.notes.current().is_empty(), "the note it staged is not taken over");
    }

    #[test]
    fn what_ran_in_a_turn_given_up_is_told_in_full_or_by_name() {
        let mut state = a_state_with("mail bob");
        let turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up(GivenUpReason::Failed { message: "boom".to_string() }, None);
        let told = state.transcript.messages().pop().unwrap();
        let text = told.content.unwrap();
        assert!(text.contains("send_mail") && text.contains("bob") && text.contains("sent"), "{text}");
        assert_eq!(told.classification, Classification::CHANNEL_DEFAULT, "as the turn");

        let mut state = a_state_with("mail bob");
        let lowered = Classification::general(caelumora_model::confidentiality::Confidentiality::Public);
        let turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up(GivenUpReason::Stopped, Some(lowered.clone()));
        let told = state.transcript.messages().pop().unwrap();
        let text = told.content.unwrap();
        assert!(text.contains("send_mail") && !text.contains("bob") && !text.contains("sent"), "{text}");
        assert_eq!(told.classification, lowered, "as the channel");
    }

    /// A turn begun on `state`, whose model called `send_mail` and `delete`
    /// in one step: `send_mail` ran, `delete` waits for its approval.
    fn a_turn_with_an_open_step(state: &mut AgentChannelState) -> Turn<'_> {
        use caelumora_model::approval::{ApprovalId, ApprovalStatus};
        use caelumora_model::tool::{ToolCallState, ToolResultStatus, ToolVisibility};
        let mut turn = Turn::begun(AgentId::default(), String::new(), state, crate::agent_state::AgentStateHandle::detached());
        let call = |id: &str, name: &str| ToolCall { id: Some(id.to_string()), function_name: name.to_string(), arguments: serde_json::json!({ "to": "bob" }), visibility: ToolVisibility::SHOWN };
        let calls = vec![call("1", "send_mail"), call("2", "delete")];
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Assistant, tool_calls: calls.clone(), ..Message::new(Classification::CHANNEL_DEFAULT) });
        let states: Vec<ToolCallState> = calls.into_iter().zip([ApprovalStatus::Approved, ApprovalStatus::Pending])
            .map(|(tool_call, status)| ToolCallState { id: ApprovalId::new(), status, tool_call, description: String::new(), result: None })
            .collect();
        turn.tool_calls().register(states);
        let (id, ran) = turn.tool_calls().to_run().remove(0);
        let mut result = ToolCallResult::of(&ran, ToolResultStatus::Ok, "sent".to_string(), Classification::CHANNEL_DEFAULT);
        result.started_at = Some(chrono::Utc::now());
        turn.tool_calls().record(id, result);
        turn
    }

    #[tokio::test]
    async fn a_cancelled_turn_is_taken_over_with_every_call_answered() {
        use caelumora_model::tool::ToolResultStatus;
        let mut state = a_state_with("mail bob, then delete");
        let turn = a_turn_with_an_open_step(&mut state);
        let committed = turn.commit(true).await;

        let messages = state.transcript.messages();
        let roles: Vec<MessageRole> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [MessageRole::User, MessageRole::Assistant, MessageRole::Tool], "the calls are answered right after they were made");
        let results: Vec<_> = messages[2].tool_results.iter().map(|r| (r.tool_name.as_str(), r.status)).collect();
        assert_eq!(results, [("send_mail", ToolResultStatus::Ok), ("delete", ToolResultStatus::NotExecuted)], "what ran is kept");
        assert!(messages[2].tool_results[1].content.contains("cancelled"), "the model is told why: {}", messages[2].tool_results[1].content);
        assert!(committed.final_messages.iter().any(|m| m.role == MessageRole::Tool), "observers learn of it too");
        let abandoned: Vec<_> = committed.ended.abandoned_calls().iter().map(|c| (c.tool_call.function_name.as_str(), c.status)).collect();
        assert_eq!(abandoned, [("delete", caelumora_model::approval::ApprovalStatus::Pending)], "only the call without a result, as it stood");
    }

    #[test]
    fn a_turn_given_up_with_an_open_step_tells_what_ran_in_it() {
        use caelumora_model::tool::ToolResultStatus;
        let mut state = a_state_with("mail bob, then delete");
        let turn = a_turn_with_an_open_step(&mut state);
        turn.give_up(GivenUpReason::Failed { message: "boom".to_string() }, None);

        let told = state.transcript.messages().pop().unwrap().content.unwrap();
        assert!(told.contains("send_mail") && !told.contains("delete"), "only what ran: {told}");
        let turns = state.transcript.given_up_turns();
        let answered: Vec<_> = turns[0].messages.last().unwrap().tool_results.iter().map(|r| r.status).collect();
        assert_eq!(answered, [ToolResultStatus::Ok, ToolResultStatus::NotExecuted], "the branch holds no call without a result");
    }

    #[test]
    fn a_turn_given_up_before_it_said_anything_leaves_nothing() {
        let mut state = a_state_with("hello");
        let turn = Turn::begun(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached());
        turn.give_up(GivenUpReason::Stopped, None);
        assert!(state.transcript.given_up_turns().is_empty());
        assert_eq!(state.transcript.entries().len(), 1);
    }

    #[tokio::test]
    async fn committing_a_turn_answers_with_its_final_messages_and_clears_its_work() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, AgentStateHandle::detached());

        let mut internal = message("working on it");
        internal.internal = true;
        let mut final_answer = message("here you go");
        final_answer.internal = false;

        turn.push_message(internal);
        turn.push_message(final_answer.clone());

        let committed = turn.commit(false).await;
        assert_eq!(committed.final_messages.len(), 1, "internal steps are not part of the answer");
        assert_eq!(committed.final_messages[0].id, final_answer.id);
        assert_eq!(agent_channel_state.transcript.messages().len(), 2, "but of the transcript");
        assert!(agent_channel_state.turn.is_none(), "nothing of the turn's work is stored with the state");
    }
}
