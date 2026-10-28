//! The tool calls of a turn's current step (issue 83): registering them with the approvals and the
//! answers they wait for, settling those a decision, an answer or the leave of whom they asked
//! settles, recording the results of those that ran, and - once every call has its result - the
//! tool message that gives them to the model.
//!
//! The calls are part of the turn's working state (`TurnState::tool_calls`),
//! stored with it so that a step resumes after a restart. [`ToolCalls`] is a
//! view on them that `Turn::tool_calls` hands out for one use at a time:
//! running a call needs the whole turn, which a longer-lived view would hold.
//! Running is the actor's - it owns the executor.

use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::prompt_catalogue::PromptTexts;
use caelumora_model::turn::TurnState;
use caelumora_model::approval::{ApprovalDecision, ApprovalId, ApprovalStatus};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::question::{Answers, QuestionError, QuestionId, Response};
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolCallState, ToolCallStatus, ToolResultStatus};
use std::mem;
use chrono::Utc;
use crate::tool::{abandoned_result, declined_result, denied_result, skipped_result, timed_out_result, unanswered_result, TurnEnd};

/// The tool calls of the current step of a turn, of the agent the turn's
/// context is sent by, called `agent_name` now; what the model is told of
/// a call answered without running is worded by `prompts` (issue 57).
pub(crate) struct ToolCalls<'t> {
    turn: &'t mut TurnState,
    agent_name: &'t str,
    prompts: PromptTexts,
}

impl<'t> ToolCalls<'t> {
    pub(crate) fn new(turn: &'t mut TurnState, agent_name: &'t str, prompts: PromptTexts) -> Self {
        Self { turn, agent_name, prompts }
    }

    /// Whether the step has calls.
    pub(crate) fn is_empty(&self) -> bool {
        self.turn.tool_calls.is_empty()
    }

    /// Takes the calls of the step over, as the actor prepared them - some
    /// answered already (issue 118) - and returns the requests for those
    /// that wait for a person (`requests`).
    pub(crate) fn register(&mut self, calls: Vec<ToolCallState>) -> Vec<ActorEvent> {
        self.turn.tool_calls = calls;
        self.requests()
    }

    /// The requests of the calls that wait for a person, as the turn makes
    /// them - the same ids every time: `ApprovalRequested` for those that
    /// wait for a decision (`TurnState::pending_approvals`),
    /// `QuestionRequested` for those that wait for answers
    /// (`TurnState::pending_questions`, issue 165).
    pub(crate) fn requests(&self) -> Vec<ActorEvent> {
        self.turn.pending_approvals().into_iter().map(ActorEvent::ApprovalRequested)
            .chain(self.turn.pending_questions().into_iter().map(ActorEvent::QuestionRequested))
            .collect()
    }

    /// Records the decision on the approval `id`, with `decided_by`, whose
    /// answers made it (issue 419), kept with the call into its result: an
    /// approved call asks its questions, if it has any, or may run; one
    /// denied or timed out is answered so at once, without running it. What
    /// the model is given for each outcome is decided in `crate::tool`
    /// (issue 15). Returns the requests the decision leads to - the
    /// questions an approved call now asks - or `None` if no call waits for
    /// that approval.
    pub(crate) fn decide(&mut self, id: ApprovalId, decision: ApprovalStatus, decided_by: Vec<ApprovalDecision>) -> Option<Vec<ActorEvent>> {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let call = self.turn.tool_calls.iter_mut().find(|c| c.approval_id() == Some(id))?;
        let ToolCallStatus::WaitingApproval { questions, .. } = &mut call.status else { return None };
        let questions = questions.take();
        call.approval = decided_by;
        let mut asks = None;
        call.status = match (decision, questions) {
            (ApprovalStatus::Approved, Some(questions)) => {
                let question = QuestionId::new();
                asks = Some(question);
                ToolCallStatus::WaitingUser { id: question, questions }
            }
            (ApprovalStatus::Approved, None) => ToolCallStatus::Ready { answers: None },
            (ApprovalStatus::Denied, _) => ToolCallStatus::Finished(ToolCallResult { approval: call.approval.clone(), ..ToolCallResult::of(&call.tool_call, ToolResultStatus::Denied, denied_result(&self.prompts), label) }),
            (ApprovalStatus::Timeout, _) => ToolCallStatus::Finished(ToolCallResult { approval: call.approval.clone(), ..ToolCallResult::of(&call.tool_call, ToolResultStatus::TimedOut, timed_out_result(&self.prompts), label) }),
        };
        Some(self.turn.pending_questions().into_iter()
            .filter(|request| Some(request.id) == asks)
            .map(ActorEvent::QuestionRequested)
            .collect())
    }

    /// Records the `responses` of `by`, called `by_name`, to the questions
    /// `id` (issue 165): the call may run with them. Returns the event that
    /// closes the questions for those they were shown to.
    ///
    /// # Errors
    /// `NotOpen` if no call waits for answers to `id`, `NotAsked` if `by`
    /// was not asked, and what `Questions::check_responses` finds. Nothing
    /// is recorded then.
    pub(crate) fn answer(&mut self, id: QuestionId, by: ActorId, by_name: String, responses: Vec<Response>) -> Result<ActorEvent, QuestionError> {
        let call = self.asking(id, by)?;
        let ToolCallStatus::WaitingUser { questions, .. } = &call.status else { return Err(QuestionError::NotOpen) };
        questions.check_responses(&responses)?;
        let closed = ActorEvent::QuestionClosed { id, shown_to: questions.shown_to() };
        let questions = questions.clone();
        call.status = ToolCallStatus::Ready { answers: Some(Answers { questions, answered_by: by, answered_by_name: by_name, responses }) };
        Ok(closed)
    }

    /// Records that `by`, called `by_name`, declined to answer the
    /// questions `id` (issue 165): the call is answered so at once, without
    /// running it, with the record of who declined - and of who approved
    /// it before, if it needed approval (issue 419). Returns the event that
    /// closes the questions.
    ///
    /// # Errors
    /// `NotOpen` and `NotAsked` as for `answer`.
    pub(crate) fn decline(&mut self, id: QuestionId, by: ActorId, by_name: String) -> Result<ActorEvent, QuestionError> {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let declined = declined_result(&self.prompts);
        let call = self.asking(id, by)?;
        let ToolCallStatus::WaitingUser { questions, .. } = &call.status else { return Err(QuestionError::NotOpen) };
        let closed = ActorEvent::QuestionClosed { id, shown_to: questions.shown_to() };
        let mut result = ToolCallResult::of(&call.tool_call, ToolResultStatus::Declined, declined, label);
        result.answers = Answers::declined(questions.clone(), by, by_name).record();
        result.approval = call.approval.clone();
        call.status = ToolCallStatus::Finished(result);
        Ok(closed)
    }

    /// Takes `participant`, who left the channel, off the questions of the
    /// step (issue 482): a call whose questions nobody asked is left to
    /// answer is answered as not executed, without running it - unanswered,
    /// not declined -, also one that still waits for its approval. Returns
    /// the events that withdraw what those calls waited for: the questions
    /// closed, the approval request withdrawn.
    pub(crate) fn left(&mut self, participant: ActorId) -> Vec<ActorEvent> {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let mut events = Vec::new();
        for call in &mut self.turn.tool_calls {
            let questions = match &mut call.status {
                ToolCallStatus::WaitingUser { questions, .. } | ToolCallStatus::WaitingApproval { questions: Some(questions), .. } => questions,
                _ => continue,
            };
            if !questions.is_asked(participant) || questions.remove_asked(participant) {
                continue;
            }
            events.extend(call.withdrawal());
            let result = ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, unanswered_result(&self.prompts), label.clone());
            finish(call, result);
        }
        events
    }

    /// The call that waits for the answers to `id`, if `by` was asked them.
    fn asking(&mut self, id: QuestionId, by: ActorId) -> Result<&mut ToolCallState, QuestionError> {
        let call = self.turn.tool_calls.iter_mut().find(|c| c.question_id() == Some(id)).ok_or(QuestionError::NotOpen)?;
        match &call.status {
            ToolCallStatus::WaitingUser { questions, .. } if questions.is_asked(by) => Ok(call),
            _ => Err(QuestionError::NotAsked),
        }
    }

    /// The calls that may run and have not yet, for the actor to run, each
    /// with its place in the step, which `record` takes, and the answers to
    /// its questions, if it asked any.
    pub(crate) fn to_run(&self) -> Vec<(usize, ToolCall, Option<Answers>)> {
        self.turn.tool_calls.iter().enumerate()
            .filter_map(|(index, c)| match &c.status {
                ToolCallStatus::Ready { answers } => Some((index, c.tool_call.clone(), answers.clone())),
                _ => None,
            })
            .collect()
    }

    /// Records `result` as what the call at `index` of the step gave, with
    /// the record of the answers it ran with (`finish`).
    pub(crate) fn record(&mut self, index: usize, result: ToolCallResult) {
        if let Some(call) = self.turn.tool_calls.get_mut(index) {
            finish(call, result);
        }
    }

    /// Answers the call at `index` of the step, which a user, `by`,
    /// interrupted before it ran (issue 276), as not executed, without
    /// running it: the turn goes on.
    pub(crate) fn skip(&mut self, index: usize, by: &str) {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        if let Some(call) = self.turn.tool_calls.get_mut(index) {
            let result = ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, skipped_result(&self.prompts, by), label);
            finish(call, result);
        }
    }

    /// Ends the step as its turn ends (issue 148): answers every call not
    /// yet finished as not executed, because the turn ended so - `end` -, and
    /// makes the tool message as `collect` does, so that no call is left
    /// unanswered and the results of those that ran are kept. Returns the
    /// calls it abandoned as they stood before, still waiting or ready, and
    /// the tool message for the turn to say, if the step had calls.
    pub(crate) fn abandon(&mut self, end: TurnEnd<'_>, internal: bool) -> (Vec<ToolCallState>, Option<Message>) {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let mut abandoned = Vec::new();
        for call in self.turn.tool_calls.iter_mut().filter(|c| !c.is_finished()) {
            abandoned.push(call.clone());
            let content = abandoned_result(&self.prompts, &end);
            let result = ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, content, label.clone());
            finish(call, result);
        }
        (abandoned, self.collect(internal))
    }

    /// Once every call has its result: the tool message that gives them to
    /// the model, for the turn to say (`Turn::say`) - `internal` like the
    /// step that made the calls - and the step's calls done. `None` while a
    /// call still waits for its approval, and for a step without calls:
    /// there is nothing to give.
    pub(crate) fn collect(&mut self, internal: bool) -> Option<Message> {
        if self.is_empty() || self.turn.tool_calls.iter().any(|c| !c.is_finished()) {
            return None;
        }
        let tool_results: Vec<_> = mem::take(&mut self.turn.tool_calls).into_iter()
            .filter_map(|c| match c.status {
                ToolCallStatus::Finished(result) => Some(result),
                _ => None,
            })
            .collect();
        // The model sees the results from here on (issue 168).
        for result in &tool_results {
            self.turn.work.see(&result.classification);
        }
        let message = Message {
            id: MessageId::new(),
            sender_id: self.turn.work.event_context.sender_id,
            sender_name: self.agent_name.to_string(),
            role: MessageRole::Tool,
            tool_results,
            content: None,
            timestamp: Utc::now(),
            internal,
            turn_id: Some(self.turn.work.turn_id),
            ..Message::new(self.turn.work.label())
        };
        Some(message)
    }
}

/// Finishes `call` with `result`, and keeps with it the record of the
/// answers it was to run with (issue 165), if it had any - unless they are
/// sensitive (`Answers::record`) -, and of who approved it (issue 419).
fn finish(call: &mut ToolCallState, mut result: ToolCallResult) {
    if let ToolCallStatus::Ready { answers: Some(answers) } = &call.status {
        result.answers = answers.record();
    }
    result.approval = call.approval.clone();
    call.status = ToolCallStatus::Finished(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::ToolMatcher;
    use caelumora_model::question::{Addressee, Question, Questions};
    use caelumora_model::tool::ToolVisibility;
    use caelumora_model::user::UserId;
    use caelumora_model::confidentiality::Classification;

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some(name.to_string()), function_name: name.to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }
    }

    fn view(turn: &mut TurnState) -> ToolCalls<'_> {
        ToolCalls::new(turn, "agent", testing::prompts())
    }

    fn a_turn() -> TurnState {
        let mut turn = TurnState::default();
        turn.work.event_context.sender_id = ActorId::Agent(AgentId::default());
        turn
    }

    fn questions_of(asked: ActorId) -> Questions {
        Questions {
            questions: vec![Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }],
            ask: vec![Addressee::Actor(asked)],
            sensitive: false,
        }
    }

    #[test]
    fn a_steps_calls_are_answered_run_and_given_back_to_the_model_in_one_message() {
        let mut turn = a_turn();

        // Registering asks for the approvals still to be given.
        let requests = view(&mut turn).register(vec![
            ToolCallState::approved(call("run")),
            ToolCallState::pending(call("ask"), String::new(), false, Some(ToolMatcher::exact(&call("ask"))), None),
        ]);
        let [ActorEvent::ApprovalRequested(request)] = requests.as_slice() else { panic!("one approval asked for: {requests:?}") };
        assert_eq!(request.tool_call.function_name, "ask");

        // Waiting for one of them, there is no tool message yet.
        let to_run = view(&mut turn).to_run();
        assert_eq!(to_run.iter().map(|(_, c, _)| c.function_name.as_str()).collect::<Vec<_>>(), ["run"]);
        view(&mut turn).record(to_run[0].0, ToolCallResult::of(&to_run[0].1, ToolResultStatus::Ok, "ran".to_string(), Classification::CHANNEL_DEFAULT));
        assert!(view(&mut turn).collect(false).is_none(), "one call still waits");

        // A denial answers the other without running it, with who denied it (issue 419).
        let bob = ApprovalDecision { by: ActorId::User(UserId::new()), name: "bob".to_string(), by_rule: None, at: Utc::now() };
        assert_eq!(view(&mut turn).decide(request.id, ApprovalStatus::Denied, vec![bob.clone()]).map(|e| e.len()), Some(0));
        assert!(view(&mut turn).decide(ApprovalId::new(), ApprovalStatus::Denied, Vec::new()).is_none(), "no such call");
        assert!(view(&mut turn).to_run().is_empty(), "nothing left to run");
        let message = view(&mut turn).collect(false).expect("every call has its result");
        let results: Vec<_> = message.tool_results.iter().map(|r| (r.tool_name.as_str(), r.status)).collect();
        assert_eq!(results, [("run", ToolResultStatus::Ok), ("ask", ToolResultStatus::Denied)]);
        assert_eq!((message.tool_results[0].approval.as_slice(), message.tool_results[1].approval.as_slice()), (&[][..], &[bob][..]), "who denied it, and nothing of a call that needed no approval");
        assert_eq!(message.role, MessageRole::Tool);
        assert!(view(&mut turn).is_empty(), "the step's calls are done");
        assert!(view(&mut turn).collect(false).is_none(), "nothing is left to give");
    }

    #[test]
    fn a_call_asks_its_questions_once_approved_and_runs_with_the_answers_of_whom_it_asked() {
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let mut turn = a_turn();
        let requests = view(&mut turn).register(vec![
            ToolCallState::pending(call("approved"), String::new(), false, Some(ToolMatcher::exact(&call("approved"))), Some(questions_of(alice))),
            ToolCallState::asking(call("asking"), questions_of(alice)),
        ]);
        let [ActorEvent::ApprovalRequested(approval), ActorEvent::QuestionRequested(asked)] = requests.as_slice() else { panic!("an approval first, the questions of the other call: {requests:?}") };
        assert_eq!(asked.tool_call.function_name, "asking");

        // Approved, the first asks its questions, under an id of their own.
        let approved_by = ApprovalDecision { by: alice, name: "alice".to_string(), by_rule: None, at: Utc::now() };
        let after = view(&mut turn).decide(approval.id, ApprovalStatus::Approved, vec![approved_by]).expect("the call waited for it");
        let [ActorEvent::QuestionRequested(asked_after)] = after.as_slice() else { panic!("its questions: {after:?}") };
        assert_eq!(asked_after.tool_call.function_name, "approved");
        assert_eq!(view(&mut turn).requests().len(), 2, "both ask now, as a resumed turn would ask again");

        // Only whom they were asked of answers, and only as they ask.
        let text = || vec![Response::Text { text: "Kaya".to_string() }];
        assert_eq!(view(&mut turn).answer(asked.id, bob, "bob".to_string(), text()).err(), Some(QuestionError::NotAsked));
        assert_eq!(view(&mut turn).answer(asked.id, alice, "alice".to_string(), vec![]).err(), Some(QuestionError::ResponseCount(1)));
        assert_eq!(view(&mut turn).answer(QuestionId::new(), alice, "alice".to_string(), text()).err(), Some(QuestionError::NotOpen));
        let closed = view(&mut turn).answer(asked.id, alice, "alice".to_string(), text()).expect("answered");
        assert!(matches!(closed, ActorEvent::QuestionClosed { id, shown_to: None } if id == asked.id), "for everyone it was shown to");
        assert_eq!(view(&mut turn).answer(asked.id, alice, "alice".to_string(), text()).err(), Some(QuestionError::NotOpen), "answered once");

        let to_run = view(&mut turn).to_run();
        let [(index, run, Some(answers))] = to_run.as_slice() else { panic!("the answered call runs with its answers: {to_run:?}") };
        assert_eq!((run.function_name.as_str(), answers.answered_by, answers.answered_by_name.as_str()), ("asking", alice, "alice"));
        assert_eq!(answers.questions, questions_of(alice), "with the questions they answer");

        // Declined, the other is answered so and never runs.
        assert_eq!(view(&mut turn).decline(asked_after.id, bob, "bob".to_string()).err(), Some(QuestionError::NotAsked));
        view(&mut turn).decline(asked_after.id, alice, "alice".to_string()).expect("declined");
        assert_eq!(view(&mut turn).to_run().len(), 1, "only the answered call");

        // The result of each keeps the record of its answers.
        view(&mut turn).record(*index, ToolCallResult::of(run, ToolResultStatus::Ok, "ran".to_string(), Classification::CHANNEL_DEFAULT));
        let result = |name: &str| turn.tool_calls.iter().find(|c| c.tool_call.function_name == name).and_then(|c| c.result()).cloned().expect("finished");
        assert_eq!(result("asking").answers.as_ref(), Some(answers), "what it ran with");
        let declined = result("approved");
        assert_eq!(declined.status, ToolResultStatus::Declined);
        assert_eq!(declined.answers.map(|a| (a.answered_by, a.responses)), Some((alice, vec![Response::Declined])), "who declined");
        assert_eq!(declined.approval.iter().map(|d| d.by).collect::<Vec<_>>(), [alice], "and who approved it before (issue 419)");
        assert!(result("asking").approval.is_empty(), "nothing of approval where none was needed");
    }

    #[test]
    fn the_answers_to_sensitive_questions_are_kept_with_no_result() {
        let alice = ActorId::User(UserId::new());
        let mut turn = a_turn();
        let sensitive = Questions { sensitive: true, ..questions_of(alice) };
        let requests = view(&mut turn).register(vec![ToolCallState::asking(call("asking"), sensitive.clone()), ToolCallState::asking(call("other"), sensitive)]);
        let [ActorEvent::QuestionRequested(answered), ActorEvent::QuestionRequested(declined)] = requests.as_slice() else { panic!("{requests:?}") };
        view(&mut turn).answer(answered.id, alice, "alice".to_string(), vec![Response::Text { text: "hunter2".to_string() }]).expect("answered");
        view(&mut turn).decline(declined.id, alice, "alice".to_string()).expect("declined");
        let to_run = view(&mut turn).to_run();
        let [(index, run, Some(_))] = to_run.as_slice() else { panic!("it runs with them: {to_run:?}") };
        view(&mut turn).record(*index, ToolCallResult::of(run, ToolResultStatus::Ok, "ran".to_string(), Classification::CHANNEL_DEFAULT));
        assert!(turn.tool_calls.iter().all(|c| c.result().is_some_and(|r| r.answers.is_none())), "neither the answers nor who declined");
    }

    #[test]
    fn questions_nobody_asked_is_left_to_answer_go_unanswered_and_their_calls_never_run() {
        let alice = ActorId::User(UserId::new());
        let bob = ActorId::User(UserId::new());
        let both = Questions { ask: vec![Addressee::Actor(alice), Addressee::Actor(bob)], ..questions_of(alice) };
        let mut turn = a_turn();
        let requests = view(&mut turn).register(vec![
            ToolCallState::asking(call("both"), both),
            ToolCallState::asking(call("alice"), questions_of(alice)),
            ToolCallState::pending(call("approving"), String::new(), false, None, Some(questions_of(alice))),
            ToolCallState::asking(call("bob"), questions_of(bob)),
        ]);
        let [ActorEvent::ApprovalRequested(approval), ActorEvent::QuestionRequested(_), ActorEvent::QuestionRequested(alone), ActorEvent::QuestionRequested(_)] = requests.as_slice() else { panic!("{requests:?}") };
        let status = |turn: &TurnState, name: &str| turn.tool_calls.iter().find(|c| c.tool_call.function_name == name).map(|c| c.status.clone()).expect("a call of the step");

        // Alice leaves: what she alone was asked goes unanswered, an approval
        // still awaited is withdrawn.
        let settled = view(&mut turn).left(alice);
        let [ActorEvent::QuestionClosed { id, .. }, ActorEvent::ApprovalRequestWithdrawn { id: withdrawn }] = settled.as_slice() else { panic!("{settled:?}") };
        assert_eq!((*id, *withdrawn), (alone.id, approval.id));
        let ToolCallStatus::Finished(unanswered) = status(&turn, "alice") else { panic!("answered without running") };
        assert_eq!(unanswered.status, ToolResultStatus::NotExecuted);
        assert!(unanswered.answers.is_none(), "nobody answered or declined");
        assert!(matches!(status(&turn, "approving"), ToolCallStatus::Finished(ToolCallResult { status: ToolResultStatus::NotExecuted, .. })), "never asked: nobody is left to answer");
        let ToolCallStatus::WaitingUser { questions, .. } = status(&turn, "both") else { panic!("still open") };
        assert_eq!(questions.asked(), [bob], "to bob alone");
        assert!(matches!(status(&turn, "bob"), ToolCallStatus::WaitingUser { .. }), "not hers");
        assert!(view(&mut turn).left(alice).is_empty(), "nothing is left of hers");

        // Bob leaves too: nobody is left to answer.
        assert_eq!(view(&mut turn).left(bob).len(), 2);
        assert!(view(&mut turn).collect(false).is_some(), "every call has its result");
    }
}
