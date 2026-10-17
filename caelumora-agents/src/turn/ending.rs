//! How a turn ends: taken over when it completes (`Turn::commit`), or
//! given up when it fails or is stopped (`Turn::give_up`, issue 145). Either
//! way its messages stay where they were said, followed by its end (issue
//! 258). Both use the turn up and return what is still needed of it: the
//! contexts its last events are sent in (`Answering`) and the tool calls it
//! abandoned.

use super::Turn;
use caelumora_runtime::actor::Answering;
use crate::turn::tool_calls::ToolCalls;
use caelumora_runtime::transport::EventSender;
use caelumora_model::turn::TurnState;
use caelumora_model::message::Message;
use caelumora_model::tool::ToolCallState;
use caelumora_model::turn::GivenUpReason;
use tracing::warn;

/// What is still needed of a turn once it is taken over (`commit`).
#[derive(Debug)]
pub(crate) struct CommittedTurn {
    /// The messages that are not internal: the turn's answer, for
    /// `TurnFinished`.
    pub(crate) final_messages: Vec<Message>,
    /// Whether the agent's state in the channel changed, to be announced
    /// once stored.
    pub(crate) channel_state_changed: bool,
    /// What its answer is sent with: the contexts the turn had (issue 85).
    pub(crate) answering: Answering,
    /// The tool calls that had no result when it ended, answered as not
    /// executed (`Turn::abandon_tool_calls`, issue 148): their approval
    /// requests are to be withdrawn.
    pub(crate) abandoned_calls: Vec<ToolCallState>,
}

impl Turn<'_> {
    /// Takes the completed turn over and ends it (issues 06, 17, 47, 62, 65):
    ///
    /// - its answer is what it said that is not internal, kept with the turn
    ///   as it was said (`TurnState::answer`, issue 417) - not read from the
    ///   transcript, of which a compaction may have summarised a part (issue
    ///   284);
    /// - records its end, `TurnFinished`;
    /// - applies the changes staged to its state in this channel, anchored at
    ///   the last message of its transcript;
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
    pub(crate) async fn commit(mut self, cancelled: bool, output: &dyn EventSender) -> CommittedTurn {
        let abandoned_calls = self.abandon_tool_calls(if cancelled { "it was cancelled" } else { "it ended" }, output).await;
        let answering = self.answering();
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let turn_id = turn.work.turn_id;
        // Every part is used up exactly once; what it said is its answer
        // (issue 417), whatever a compaction left of it in the transcript
        // (issue 284).
        let TurnState { memory, notes, private_notes, answer: final_messages, .. } = turn;
        let channel_state_changed = self.agent_channel_state.finish_turn(turn_id, notes, private_notes);
        if let Err(e) = self.agent_state.apply(memory, self.agent_channel_state.last_message.unwrap_or_default()).await {
            warn!("Agent {}: the changes of its turn to its own state are lost: {:#}", self.agent_id, e);
        }
        CommittedTurn { final_messages, channel_state_changed, answering, abandoned_calls }
    }

    /// Gives the turn up after it failed or was stopped, for `reason`
    /// (issues 145, 258): what it said stays where it was said - in the
    /// transcript since it said it (issue 260), and in its channel's
    /// history -, followed by its end, `TurnGivenUp`. Nothing it staged is
    /// applied. A step whose calls do not all have their result yet is ended
    /// first (`abandon_tool_calls`).
    ///
    /// What its tools did stays done, and its calls stand in its messages;
    /// the model is told why the turn ended where its end stands, and of a
    /// turn given up as `Lowered` is given only which tools it called
    /// (`ContextResolver`).
    ///
    /// Returns what its last events are sent with, and the calls it
    /// abandoned.
    pub(crate) async fn give_up(mut self, reason: GivenUpReason, output: &dyn EventSender) -> (Answering, Vec<ToolCallState>) {
        let abandoned_calls = self.abandon_tool_calls(reason.cause(), output).await;
        let answering = self.answering();
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        self.agent_channel_state.give_up_turn(turn.work.turn_id, reason);
        (answering, abandoned_calls)
    }

    /// Abandons the tool calls of the current step as the turn ends,
    /// `because` - why it ends (issue 148): those without a result are
    /// answered as not executed, and the tool message is said like any
    /// other, internal like the step (issue 258). No call of the turn is
    /// left unanswered in the transcript, and the results of those that ran
    /// are not lost with the turn's working state. Returns the calls
    /// abandoned.
    ///
    /// The turn is ending: a failed send is logged, not returned. The
    /// message is kept all the same (`Turn::say`).
    async fn abandon_tool_calls(&mut self, because: &str, output: &dyn EventSender) -> Vec<ToolCallState> {
        let internal = self.in_plan_step();
        let Some(turn) = self.agent_channel_state.turn.as_mut() else { return Vec::new() };
        let (abandoned, message) = ToolCalls::new(turn, &self.agent_name).abandon(because, internal);
        if let Some(message) = message {
            if let Err(e) = self.say(message, output).await {
                warn!("Agent {}: the answers to the calls it abandoned were not published: {:#}", self.agent_id, e);
            }
        }
        abandoned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::NoteKind;
    use crate::turn::tests::message;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::message::MessageId;
    use caelumora_model::message::MessageRole;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::tool::{ToolCall, ToolCallResult};
    use caelumora_runtime::transport::Discard;
    use caelumora_model::actor::ActorEvent;
    use caelumora_runtime::transport::memory::InMemoryEventSender;
    use chrono::Utc;
    use crate::message::testing::given_up_turns;
    use serde_json::Value;

    /// A turn begun on `state`, whose model called `tool` with `arguments`;
    /// the call ran if `ran`, and a note was staged.
    fn a_turn_that_called<'a>(state: &'a mut AgentChannelState, tool: &str, ran: bool) -> Turn<'a> {
        use caelumora_model::note::{NoteChange, NoteId};
        use caelumora_model::tool::{ToolResultStatus, ToolVisibility};
        let mut turn = Turn::begun(AgentId::default(), state);
        let call = ToolCall { id: Some("1".to_string()), function_name: tool.to_string(), arguments: serde_json::json!({ "to": "bob" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let mut result = ToolCallResult::of(&call, ToolResultStatus::Ok, "sent".to_string(), Classification::CHANNEL_DEFAULT);
        result.started_at = ran.then(Utc::now);
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Assistant, tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Tool, tool_results: vec![result], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.stage_note(NoteKind::Public, NoteChange::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT });
        turn
    }

    fn a_state_with(content: &str) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let message = Message { id: MessageId::new(), role: MessageRole::User, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.transcript.push(message);
        state
    }

    #[tokio::test]
    async fn a_turn_given_up_keeps_what_it_said_and_nothing_it_staged_is_applied() {
        let mut state = a_state_with("mail bob");
        let turn = a_turn_that_called(&mut state, "send_mail", false);
        turn.give_up(GivenUpReason::Left, &Discard).await;

        assert!(state.turn.is_none(), "the turn is over");
        assert_eq!(state.transcript.messages().len(), 3, "what it said is not unsaid (issue 258)");
        assert_eq!(state.transcript.all_entries().count(), 4, "followed by its end, and nothing branches");
        let turns = given_up_turns(&state);
        assert_eq!(turns.len(), 1);
        assert_eq!((turns[0].1.clone(), turns[0].2.len()), (GivenUpReason::Left, 2));
        assert!(state.notes.current().is_empty(), "the note it staged is not taken over");
    }

    /// What the model of the next turn begun on `state` is given.
    fn given_next(state: &mut AgentChannelState) -> Vec<Message> {
        let turn = Turn::begun(AgentId::default(), state);
        turn.full_context().expect("resolved").0
    }

    #[tokio::test]
    async fn the_model_is_told_why_a_turn_was_given_up_and_of_a_lowered_one_only_which_tools_it_called() {
        let mut state = a_state_with("mail bob");
        let turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up(GivenUpReason::Failed { message: "boom".to_string() }, &Discard).await;
        let given = given_next(&mut state);
        assert_eq!(given[1].tool_calls[0].arguments, serde_json::json!({ "to": "bob" }), "its call in full");
        assert_eq!(given[2].tool_results[0].content, "sent");
        let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
        assert!(told.contains("because it failed") && !told.contains("boom"), "where its end stands, without what its users were told: {told}");

        let mut state = a_state_with("mail bob");
        let lowered = Classification::general(Confidentiality::Public);
        let turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up(GivenUpReason::Lowered { channel: lowered.clone() }, &Discard).await;
        let given = given_next(&mut state);
        assert_eq!((given[1].tool_calls[0].function_name.as_str(), &given[1].tool_calls[0].arguments), ("send_mail", &Value::Null), "only which tool it called");
        assert!(given[2].tool_results[0].content.is_empty(), "nothing of what it returned");
        assert_eq!(given[1].classification, lowered, "as the channel is now");
        let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
        assert!(told.contains("withheld"), "{told}");
        assert_eq!(state.transcript.messages()[1].tool_calls[0].arguments, serde_json::json!({ "to": "bob" }), "the record keeps it all");
    }

    /// A turn begun on `state`, whose model called `send_mail` and `delete`
    /// in one step: `send_mail` ran, `delete` waits for its approval.
    fn a_turn_with_an_open_step(state: &mut AgentChannelState) -> Turn<'_> {
        use caelumora_model::approval::ToolMatcher;
        use caelumora_model::tool::{ToolCallState, ToolResultStatus, ToolVisibility};
        let mut turn = Turn::begun(AgentId::default(), state);
        let call = |id: &str, name: &str| ToolCall { id: Some(id.to_string()), function_name: name.to_string(), arguments: serde_json::json!({ "to": "bob" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let calls = vec![call("1", "send_mail"), call("2", "delete")];
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Assistant, tool_calls: calls.clone(), ..Message::new(Classification::CHANNEL_DEFAULT) });
        let states = vec![
            ToolCallState::approved(calls[0].clone()),
            ToolCallState::pending(calls[1].clone(), String::new(), false, Some(ToolMatcher::exact(&calls[1])), None),
        ];
        turn.tool_calls().register(states);
        let (id, ran, _) = turn.tool_calls().to_run().remove(0);
        let mut result = ToolCallResult::of(&ran, ToolResultStatus::Ok, "sent".to_string(), Classification::CHANNEL_DEFAULT);
        result.started_at = Some(Utc::now());
        turn.tool_calls().record(id, result);
        turn
    }

    #[tokio::test]
    async fn a_cancelled_turn_is_taken_over_with_every_call_answered() {
        use caelumora_model::tool::ToolResultStatus;
        let mut state = a_state_with("mail bob, then delete");
        let turn = a_turn_with_an_open_step(&mut state);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let committed = turn.commit(true, &InMemoryEventSender::new(tx)).await;

        let messages = state.transcript.messages();
        let roles: Vec<MessageRole> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [MessageRole::User, MessageRole::Assistant, MessageRole::Tool], "the calls are answered right after they were made");
        let results: Vec<_> = messages[2].tool_results.iter().map(|r| (r.tool_name.as_str(), r.status)).collect();
        assert_eq!(results, [("send_mail", ToolResultStatus::Ok), ("delete", ToolResultStatus::NotExecuted)], "what ran is kept");
        assert!(messages[2].tool_results[1].content.contains("cancelled"), "the model is told why: {}", messages[2].tool_results[1].content);
        assert!(committed.final_messages.iter().any(|m| m.role == MessageRole::Tool), "observers learn of it too");
        let said = rx.try_recv().expect("the answers are said as the turn ends (issue 258)");
        let ActorEvent::CompleteMessageGenerated(said) = said.event else { panic!("said as a complete message: {said:?}") };
        assert_eq!(said.id, messages[2].id, "the message the transcript keeps");
        let abandoned: Vec<_> = committed.abandoned_calls.iter().map(|c| (c.tool_call.function_name.as_str(), c.approval_id().is_some())).collect();
        assert_eq!(abandoned, [("delete", true)], "only the call without a result, as it stood: waiting for its approval");
    }

    #[tokio::test]
    async fn a_turn_given_up_with_an_open_step_answers_it() {
        use caelumora_model::tool::ToolResultStatus;
        let mut state = a_state_with("mail bob, then delete");
        let turn = a_turn_with_an_open_step(&mut state);
        turn.give_up(GivenUpReason::Failed { message: "boom".to_string() }, &Discard).await;

        let turns = given_up_turns(&state);
        let answered: Vec<_> = turns[0].2.last().unwrap().tool_results.iter().map(|r| r.status).collect();
        assert_eq!(answered, [ToolResultStatus::Ok, ToolResultStatus::NotExecuted], "no call is left without a result");
    }

    #[tokio::test]
    async fn a_turn_given_up_before_it_said_anything_leaves_its_end() {
        let mut state = a_state_with("hello");
        let turn = Turn::begun(AgentId::default(), &mut state);
        turn.give_up(GivenUpReason::Left, &Discard).await;
        assert_eq!(given_up_turns(&state).len(), 1, "one rule for every turn (issue 258)");
        assert!(given_up_turns(&state)[0].2.is_empty());
        assert_eq!(state.transcript.entries().len(), 2);
    }

    #[tokio::test]
    async fn committing_a_turn_answers_with_its_final_messages_and_clears_its_work() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let mut internal = message("working on it");
        internal.internal = true;
        let mut final_answer = message("here you go");
        final_answer.internal = false;

        turn.push_message(internal);
        turn.push_message(final_answer.clone());

        let committed = turn.commit(false, &Discard).await;
        assert_eq!(committed.final_messages.len(), 1, "internal steps are not part of the answer");
        assert_eq!(committed.final_messages[0].id, final_answer.id);
        assert_eq!(agent_channel_state.transcript.messages().len(), 2, "but of the transcript");
        assert!(agent_channel_state.turn.is_none(), "nothing of the turn's work is stored with the state");
    }

    #[tokio::test]
    async fn a_turn_compacted_while_under_way_answers_with_all_it_said_and_nothing_else() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);
        let first = message("first");
        let second = message("second");
        turn.push_message(first.clone());
        turn.push_message(second.clone());
        let kept_from = turn.situation().agent_channel_state().transcript.digest();
        turn.compact(vec![(message("summary of first"), None)], kept_from).expect("compacted");
        let third = message("third");
        turn.push_message(third.clone());

        let committed = turn.commit(false, &Discard).await;
        let answer: Vec<MessageId> = committed.final_messages.iter().map(|m| m.id).collect();
        assert_eq!(answer, [first.id, second.id, third.id], "what it said, the part summarised included, and not the summary (issue 284)");
    }
}
