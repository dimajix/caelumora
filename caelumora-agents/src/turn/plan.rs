//! The plan in play in a turn: where it stands, and how it moves on - the
//! instruction for each step placed for the model, the final answer asked
//! for once every step is done.

use super::Turn;
use caelumora_model::actor::ActorEvent;
use caelumora_model::digest::{prompt, Digest};
use caelumora_model::message::MessageRole;
use caelumora_model::plan::{Plan, PlanProgress, PlanState, PlanStep};
use std::cmp::Ordering;
use std::collections::HashSet;

impl Turn<'_> {
    pub fn set_plan(&mut self, plan: PlanState) {
        self.current_mut().plan = Some(plan);
    }

    pub fn plan(&self) -> Option<&PlanState> {
        self.current().plan.as_ref()
    }

    pub fn take_plan(&mut self) -> Option<PlanState> {
        self.current_mut().plan.take()
    }

    /// Where the plan in play stands, `None` without one. A `PlanState` is
    /// only ever constructed after its `create_plan` tool call has already
    /// been through the generic tool-call approval gate, so its mere
    /// presence implies "approved and executing".
    pub(crate) fn plan_progress(&self) -> Option<PlanProgress> {
        self.current().plan.as_ref().map(|p| p.progress)
    }

    /// Whether what the model produces now is part of executing a plan -
    /// a step under way, not the final answer after the last one - and so
    /// internal: kept in the transcript, not in the history, and published
    /// as intermediate.
    pub(crate) fn in_plan_step(&self) -> bool {
        matches!(self.plan_progress(), Some(PlanProgress::Step(_)))
    }

    fn set_plan_progress(&mut self, progress: PlanProgress) {
        if let Some(plan) = self.current_mut().plan.as_mut() {
            plan.progress = progress;
        }
    }

    /// Moves the plan on to step `next`: gives the model the instruction for
    /// it and returns the `PlanStepStarted` to announce - or, with no such
    /// step, gives it the instruction to write the final answer, which is no
    /// step and announces nothing (an empty plan finalizes at once). Called
    /// when a plan is accepted or extended, and each time a step converges.
    /// The instructions are system text placed after the turn's messages so
    /// far (`place_turn_prompt`, issue 49): where the model keeps system
    /// messages only at the start, they are given as user messages.
    pub(crate) fn advance_plan(&mut self, next: usize) -> Option<ActorEvent> {
        let plan = self.plan()?.plan.clone();
        match plan.steps.get(next) {
            Some(step) => {
                self.place_turn_prompt(MessageRole::System, plan_step_instruction(next + 1, plan.steps.len(), step));
                self.set_plan_progress(PlanProgress::Step(next));
                Some(ActorEvent::PlanStepStarted {
                    turn_id: self.turn_id(),
                    step_index: next,
                    total_steps: plan.steps.len(),
                    description: step.description.clone(),
                })
            }
            None => {
                self.place_turn_prompt(MessageRole::System, plan_finalize_instruction(&plan));
                self.set_plan_progress(PlanProgress::Finalizing);
                None
            }
        }
    }

    /// Names the plan in play again after a compaction (issue 284), whose
    /// replacement ends at `replaced` - where the parts placed after what it
    /// replaced were carried. If an instruction of the plan was among them,
    /// its `create_plan` call was replaced as well, and the model no longer
    /// knows its plan: the instructions carried there are dropped, and the
    /// current one is replaced, where it stands, by the plan's overview -
    /// which steps are done, which is under way - followed by it. Nothing
    /// without a plan, or with every instruction of it kept where it was.
    ///
    /// The plan's parts are told by their text, which is the plan's state's
    /// (`plan_texts`); a part placed for a plan before it was extended is
    /// not, and stays where it was carried.
    pub(super) fn name_plan_again(&mut self, replaced: Option<Digest>) {
        let Some(PlanState { plan, progress }) = self.plan().cloned() else { return };
        let texts: HashSet<Digest> = plan_texts(&plan).iter().map(|text| prompt(text)).collect();
        let prompts = &self.current().work.context.prompts;
        let of_plan: Vec<usize> = prompts.iter().enumerate().filter(|(_, part)| texts.contains(&part.text)).map(|(at, _)| at).collect();
        let Some(&current) = of_plan.last() else { return };
        if !of_plan.iter().any(|&at| prompts[at].after == replaced) {
            return;
        }
        let after = prompts[current].after;
        let dropped: HashSet<usize> = of_plan.into_iter().filter(|&at| at == current || prompts[at].after == replaced).collect();
        let kept = prompts.iter().enumerate().filter(|(at, _)| !dropped.contains(at)).map(|(_, part)| part.clone()).collect();
        self.current_mut().work.context.prompts = kept;
        let label = self.label();
        let overview = self.keep_prompt(MessageRole::System, plan_overview_instruction(&plan, progress), after, Some(label));
        self.current_mut().work.context.prompts.push(overview);
    }

    /// Moves the plan on after the model answered without calling a tool:
    /// the step under way is finished - announced as `PlanStepFinished` -
    /// and the next one begins, or the final answer is asked for
    /// (`advance_plan`); after the final answer the plan is over. Returns
    /// the events to announce, in order; none without a plan.
    pub(crate) fn plan_converged(&mut self) -> Vec<ActorEvent> {
        let Some(plan) = self.plan() else { return Vec::new() };
        match plan.progress {
            PlanProgress::Step(step_index) => {
                let total_steps = plan.plan.steps.len();
                let description = plan.plan.steps.get(step_index).map(|s| s.description.clone()).unwrap_or_default();
                let mut events = vec![ActorEvent::PlanStepFinished { turn_id: self.turn_id(), step_index, total_steps, description }];
                events.extend(self.advance_plan(step_index + 1));
                events
            }
            PlanProgress::Finalizing => {
                self.take_plan();
                Vec::new()
            }
        }
    }
}

/// The instruction to work on step `step_number` of `total_steps`.
fn plan_step_instruction(step_number: usize, total_steps: usize, step: &PlanStep) -> String {
    format!(
        "Step {} of {}: {}\n\nWork on this step now, using any tools you need. \
         When you are finished, describe the outcome.",
        step_number, total_steps, step.description
    )
}

/// The instruction to write the final answer once every step of `plan` is
/// done.
fn plan_finalize_instruction(plan: &Plan) -> String {
    let recap = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You have completed every step of the plan:\n{}\n\nNow provide your final, \
         complete answer to the user's original request, incorporating what you learned.",
        recap
    )
}

/// The plan named again after a compaction (issue 284): its steps, each
/// marked done, under way or to do, then the instruction for the step under
/// way - or, finalizing, the instruction to write the final answer, which
/// recaps the steps itself.
fn plan_overview_instruction(plan: &Plan, progress: PlanProgress) -> String {
    let PlanProgress::Step(current) = progress else { return plan_finalize_instruction(plan) };
    let Some(step) = plan.steps.get(current) else { return plan_finalize_instruction(plan) };
    let steps = plan.steps.iter()
        .enumerate()
        .map(|(i, s)| {
            let mark = match i.cmp(&current) {
                Ordering::Less => "done",
                Ordering::Equal => "under way",
                Ordering::Greater => "to do",
            };
            format!("{}. [{}] {}", i + 1, mark, s.description)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are carrying out a plan: {}\n{}\n\n{}",
        plan.summary, steps, plan_step_instruction(current + 1, plan.steps.len(), step)
    )
}

/// Every text `plan` is given to the model as: each step's instruction,
/// the one to finalize, and its overview at each step.
fn plan_texts(plan: &Plan) -> Vec<String> {
    let total = plan.steps.len();
    plan.steps.iter()
        .enumerate()
        .flat_map(|(i, step)| [plan_step_instruction(i + 1, total, step), plan_overview_instruction(plan, PlanProgress::Step(i))])
        .chain([plan_finalize_instruction(plan)])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_runtime::transport::Discard;
    use crate::turn::tests::{message, seen};
    use caelumora_model::actor::ActorId;
    use caelumora_model::actor::EventContext;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::context::ModelProfile;
    use caelumora_model::context::{ContextItem, PromptPart};
    use caelumora_model::message::Message;
    use caelumora_model::message::MessageId;
    use caelumora_model::plan::PlanId;
    use caelumora_model::security::SecurityContext;

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

    fn plan_of(steps: &[&str]) -> PlanState {
        PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: steps.iter().map(|d| PlanStep { description: d.to_string() }).collect(),
            },
        }
    }

    /// What the model of `turn` is given, as role and text.
    fn given(turn: &Turn) -> Vec<(MessageRole, String)> {
        turn.full_context().unwrap().0.into_iter().map(|m| (m.role, m.content.unwrap_or_default())).collect()
    }

    #[test]
    fn advance_plan_moves_to_the_next_step_when_one_remains() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { role: MessageRole::User, content: Some("do it".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) });
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(1);

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Step(1));
        assert!(turn.agent_channel_state().transcript.messages().iter().all(|m| m.turn_id != Some(turn.turn_id())), "the instruction is no message of the turn");
        let (role, text) = given(&turn).pop().expect("the instruction is given to the model");
        assert_eq!(role, MessageRole::User, "a system message in the middle, given as a user message");
        assert!(text.contains("second"));
        match started {
            Some(ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description }) => {
                assert_eq!((step_index, total_steps, description.as_str()), (1, 2, "second"));
                assert_eq!(turn_id, turn.turn_id(), "of the turn");
            }
            other => panic!("expected PlanStepStarted, got {other:?}"),
        }
    }

    #[test]
    fn advance_plan_finalizes_once_steps_are_exhausted() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(2);

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
        let (_, text) = given(&turn).pop().expect("the instruction to finalize is given to the model");
        assert!(text.contains("completed every step"));
        assert!(started.is_none(), "finalization is not a step, so nothing is announced");
    }

    #[tokio::test]
    async fn plan_instructions_are_given_where_they_were_placed_and_not_kept_for_later_turns() {
        use caelumora_model::context::SystemMessages;
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), sender_id: ActorId::Agent(agent), role, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { sender_id: ActorId::None, ..text(MessageRole::User, "do it") });
        let (step, answer) = (text(MessageRole::Assistant, "step done"), text(MessageRole::Assistant, "all done"));
        let keeps_system = ModelProfile { system_messages: SystemMessages::Anywhere, ..ModelProfile::default() };
        {
            state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), Vec::new(), keeps_system);
            let mut turn = Turn::under_way(agent, &mut state);
            turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: None, source: None }], Vec::new());
            turn.set_plan(plan_of(&["only"]));
            turn.advance_plan(0);
            let mut internal = step.clone();
            internal.internal = true;
            turn.push_message(internal);
            turn.advance_plan(1);
            let roles = given(&turn).into_iter().map(|(role, _)| role).collect::<Vec<_>>();
            use MessageRole::{Assistant, System, User};
            assert_eq!(roles, [System, User, System, Assistant, System], "a model that keeps them in place is given system messages");
            turn.push_message(answer.clone());
            turn.commit(false, &Discard).await;
        }

        let recorded = seen(&state, agent, answer.id).expect("the agent produced it");
        let order = recorded.iter().map(|item| match item {
            ContextItem::Prompt { text, .. } => text.lines().next().unwrap_or_default().to_string(),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect::<Vec<_>>();
        assert_eq!(order, ["Be brief.", "do it", "Step 1 of 1: only", "step done", "You have completed every step of the plan:"],
            "each instruction where it was given");
        assert_eq!(state.transcript.messages().len(), 3, "the instructions are not in the transcript");

        state.begin_turn(EventContext::default(), SecurityContext::default(), Classification::default(), Vec::new(), keeps_system);
        let mut turn = Turn::under_way(agent, &mut state);
        turn.set_context(Vec::new(), Vec::new());
        assert_eq!(given(&turn).len(), 3, "a later turn is not given them");
    }

    #[test]
    fn advance_plan_finalizes_immediately_for_an_empty_plan() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_plan(plan_of(&[]));

        assert!(turn.advance_plan(0).is_none());
        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
    }

    #[test]
    fn plan_round_trips_through_set_and_take() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::begun(AgentId::default(), &mut agent_channel_state);

        assert!(ctx.plan_progress().is_none());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan_progress().is_some());

        let taken = ctx.take_plan();
        assert!(taken.is_some());
        assert!(ctx.plan_progress().is_none());
    }

    #[test]
    fn a_converged_answer_finishes_its_step_and_the_last_one_asks_for_the_final_answer() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        assert!(turn.plan_converged().is_empty(), "no plan, nothing to move on");
        turn.set_plan(plan_of(&["first", "second"]));
        assert!(turn.in_plan_step());

        let kinds = |events: Vec<ActorEvent>| events.iter().map(ActorEvent::kind).collect::<Vec<_>>();
        assert_eq!(kinds(turn.plan_converged()), ["PlanStepFinished", "PlanStepStarted"]);
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Step(1)));
        assert_eq!(kinds(turn.plan_converged()), ["PlanStepFinished"], "the final answer is no step");
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Finalizing));
        assert!(!turn.in_plan_step(), "the final answer is not internal");
        assert!(turn.plan_converged().is_empty());
        assert!(turn.plan_progress().is_none(), "the plan is over");
    }

    #[test]
    fn a_new_turn_clears_a_stale_plan() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::begun(AgentId::default(), &mut agent_channel_state);

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        let ctx = Turn::begun(AgentId::default(), &mut agent_channel_state);
        assert!(ctx.plan().is_none());
    }

    #[test]
    fn a_plan_whose_instructions_were_replaced_is_named_again_where_its_step_stood() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { role: MessageRole::User, content: Some("do it".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) });
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));
        turn.advance_plan(0);
        turn.push_message(message("did the first"));
        turn.advance_plan(1);
        turn.push_message(message("working on the second"));
        let at_first = turn.agent_channel_state().transcript.entries().next().map(|node| node.digest);
        turn.compact(vec![(message("summary"), None)], at_first).expect("compacted");
        assert_eq!(given(&turn).len(), 6, "nothing of the plan was replaced, so it is not named again: {:?}", given(&turn));

        // From its last message: the head is where the compaction's part ends.
        let kept_from = turn.agent_channel_state().transcript.entries().rev().find(|node| node.entry.message().is_some()).map(|node| node.digest);
        turn.compact(vec![(message("summary again"), None)], kept_from).expect("compacted");
        let texts: Vec<String> = given(&turn).into_iter().map(|(_, text)| text).collect();
        assert_eq!(texts.len(), 3, "the summary, the plan named again, what was kept: {texts:?}");
        assert_eq!(texts[0], "summary again");
        assert!(texts[1].starts_with("You are carrying out a plan: do the thing\n1. [done] first\n2. [under way] second\n\nStep 2 of 2: second"), "{}", texts[1]);
        assert_eq!(texts[2], "working on the second");
    }
}
