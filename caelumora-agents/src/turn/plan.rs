//! The plan in play in a turn: where it stands, and how it moves on - the
//! instruction for each step placed for the model, the final answer asked
//! for once every step is done.

use super::Turn;
use crate::prompt::NAMESPACE;
use caelumora_model::actor::ActorEvent;
use caelumora_model::digest::{prompt, Digest};
use caelumora_model::message::MessageRole;
use caelumora_model::plan::{Plan, PlanProgress, PlanState, PlanStep};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use serde::Serialize;
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
    /// step and announces nothing (an empty plan finalizes at once). Either
    /// may call the model as often as a turn without a plan (issue 412).
    /// Called when a plan is accepted or extended, and each time a step
    /// converges.
    /// The instructions are system text placed after the turn's messages so
    /// far (`place_turn_prompt`, issue 49): where the model keeps system
    /// messages only at the start, they are given as user messages.
    ///
    /// # Errors
    /// `PromptError` if the instruction cannot be had; nothing changes then.
    pub(crate) fn advance_plan(&mut self, next: usize) -> Result<Option<ActorEvent>, PromptError> {
        let Some(plan) = self.plan().map(|state| state.plan.clone()) else { return Ok(None) };
        let texts = self.situation().texts();
        let instruction = match plan.steps.get(next) {
            Some(step) => plan_step_instruction(&texts, next + 1, plan.steps.len(), step)?,
            None => plan_finalize_instruction(&texts, &plan)?,
        };
        self.count_model_calls_anew();
        self.place_turn_prompt(MessageRole::System, instruction);
        Ok(match plan.steps.get(next) {
            Some(step) => {
                self.set_plan_progress(PlanProgress::Step(next));
                Some(ActorEvent::PlanStepStarted {
                    turn_id: self.turn_id(),
                    step_index: next,
                    total_steps: plan.steps.len(),
                    description: step.description.clone(),
                })
            }
            None => {
                self.set_plan_progress(PlanProgress::Finalizing);
                None
            }
        })
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
    /// not, and stays where it was carried. Both are worded before anything
    /// is compacted (`plan_wording`): `wording`.
    pub(super) fn name_plan_again(
        &mut self,
        replaced: Option<Digest>,
        wording: Option<PlanWording>,
    ) {
        let Some((texts, overview)) = wording else { return };
        let prompts = &self.current().work.context.frame.prompts;
        let of_plan: Vec<usize> = prompts
            .iter()
            .enumerate()
            .filter(|(_, part)| texts.contains(&part.text))
            .map(|(at, _)| at)
            .collect();
        let Some(&current) = of_plan.last() else { return };
        if !of_plan.iter().any(|&at| prompts[at].after == replaced) {
            return;
        }
        let after = prompts[current].after;
        let dropped: HashSet<usize> = of_plan
            .into_iter()
            .filter(|&at| at == current || prompts[at].after == replaced)
            .collect();
        let kept = prompts
            .iter()
            .enumerate()
            .filter(|(at, _)| !dropped.contains(at))
            .map(|(_, part)| part.clone())
            .collect();
        self.current_mut().work.context.frame.prompts = kept;
        let label = self.situation().label();
        let overview = self.keep_prompt(MessageRole::System, overview, after, Some(label));
        self.current_mut().work.context.frame.prompts.push(overview);
    }

    /// What naming the plan in play again takes (`name_plan_again`), worded
    /// before the context is compacted: the digests of every text of the
    /// plan, and its overview where it stands; `None` without a plan.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub(super) fn plan_wording(&self) -> Result<Option<PlanWording>, PromptError> {
        let Some(PlanState { plan, progress }) = self.plan() else { return Ok(None) };
        let prompts = &self.situation().texts();
        let texts = plan_texts(prompts, plan)?.iter().map(|text| prompt(text)).collect();
        Ok(Some((texts, plan_overview_instruction(prompts, plan, *progress)?)))
    }

    /// Moves the plan on after the model answered without calling a tool:
    /// the step under way is finished - announced as `PlanStepFinished` -
    /// and the next one begins, or the final answer is asked for
    /// (`advance_plan`); after the final answer the plan is over. Returns
    /// the events to announce, in order; none without a plan.
    ///
    /// # Errors
    /// As `advance_plan`.
    pub(crate) fn plan_converged(&mut self) -> Result<Vec<ActorEvent>, PromptError> {
        let Some(plan) = self.plan() else { return Ok(Vec::new()) };
        match plan.progress {
            PlanProgress::Step(step_index) => {
                let total_steps = plan.plan.steps.len();
                let description = plan
                    .plan
                    .steps
                    .get(step_index)
                    .map(|s| s.description.clone())
                    .unwrap_or_default();
                let mut events = vec![ActorEvent::PlanStepFinished {
                    turn_id: self.turn_id(),
                    step_index,
                    total_steps,
                    description,
                }];
                events.extend(self.advance_plan(step_index + 1)?);
                Ok(events)
            }
            PlanProgress::Finalizing => {
                self.take_plan();
                Ok(Vec::new())
            }
        }
    }
}

/// The digests of every text of a plan, and its overview where it stands
/// (`Turn::plan_wording`).
pub(super) type PlanWording = (HashSet<Digest>, String);

/// The text `key` of `prompts` for a plan (issue 57), rendered with
/// `values`.
fn plan_text(
    prompts: &PromptTexts,
    key: &str,
    values: impl Serialize,
) -> Result<String, PromptError> {
    prompts.render(&format!("{NAMESPACE}.plan.{key}"), values)
}

/// The instruction to work on step `step_number` of `total_steps`.
fn plan_step_instruction(
    prompts: &PromptTexts,
    step_number: usize,
    total_steps: usize,
    step: &PlanStep,
) -> Result<String, PromptError> {
    plan_text(
        prompts,
        "step",
        context! { number => step_number, total => total_steps, description => step.description },
    )
}

/// The instruction to write the final answer once every step of `plan` is
/// done.
fn plan_finalize_instruction(prompts: &PromptTexts, plan: &Plan) -> Result<String, PromptError> {
    plan_text(
        prompts,
        "finalize",
        context! { steps => plan.steps.iter().map(|s| s.description.as_str()).collect::<Vec<_>>() },
    )
}

/// The plan named again after a compaction (issue 284): its steps, each
/// marked done, under way or to do, then the instruction for the step under
/// way - or, finalizing, the instruction to write the final answer, which
/// recaps the steps itself.
fn plan_overview_instruction(
    prompts: &PromptTexts,
    plan: &Plan,
    progress: PlanProgress,
) -> Result<String, PromptError> {
    let PlanProgress::Step(current) = progress else {
        return plan_finalize_instruction(prompts, plan);
    };
    let Some(step) = plan.steps.get(current) else {
        return plan_finalize_instruction(prompts, plan);
    };
    let steps: Vec<_> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let state = match i.cmp(&current) {
                Ordering::Less => "done",
                Ordering::Equal => "under_way",
                Ordering::Greater => "to_do",
            };
            context! { state, description => s.description }
        })
        .collect();
    let instruction = plan_step_instruction(prompts, current + 1, plan.steps.len(), step)?;
    plan_text(prompts, "overview", context! { summary => plan.summary, steps, instruction })
}

/// Every text `plan` is given to the model as: each step's instruction,
/// the one to finalize, and its overview at each step.
fn plan_texts(prompts: &PromptTexts, plan: &Plan) -> Result<Vec<String>, PromptError> {
    let total = plan.steps.len();
    let mut texts = Vec::with_capacity(2 * total + 1);
    for (i, step) in plan.steps.iter().enumerate() {
        texts.push(plan_step_instruction(prompts, i + 1, total, step)?);
        texts.push(plan_overview_instruction(prompts, plan, PlanProgress::Step(i))?);
    }
    texts.push(plan_finalize_instruction(prompts, plan)?);
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::tests::{message, seen};
    use caelumora_model::actor::ActorId;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::context::ModelProfile;
    use caelumora_model::context::{ContextItem, PromptPart};
    use caelumora_model::message::Message;
    use caelumora_model::message::MessageId;
    use caelumora_model::plan::PlanId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::work::Work;
    use caelumora_runtime::transport::Discard;
    use futures::executor;

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
    fn given(turn: &mut Turn) -> Vec<(MessageRole, String)> {
        executor::block_on(turn.full_context())
            .unwrap()
            .0
            .into_iter()
            .map(|m| (m.role, m.content.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn advance_plan_moves_to_the_next_step_when_one_remains() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message {
            role: MessageRole::User,
            content: Some("do it".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        });
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(1).unwrap();

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Step(1));
        assert!(
            turn.situation()
                .agent_channel_state()
                .transcript
                .messages()
                .iter()
                .all(|m| m.turn_id != Some(turn.turn_id())),
            "the instruction is no message of the turn"
        );
        let (role, text) = given(&mut turn).pop().expect("the instruction is given to the model");
        assert_eq!(
            role,
            MessageRole::User,
            "a system message in the middle, given as a user message"
        );
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
    fn each_step_of_a_plan_and_its_final_answer_may_call_the_model_as_often_as_a_turn() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));
        let use_up = |turn: &mut Turn| {
            while turn.may_call_model() {
                turn.count_model_call();
            }
        };

        use_up(&mut turn);
        turn.advance_plan(0).unwrap();
        assert!(turn.may_call_model(), "the first step begins with calls of its own");
        use_up(&mut turn);
        turn.try_activity("compaction");
        turn.plan_converged().unwrap();
        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Step(1));
        assert!(turn.may_call_model(), "so does the second");
        assert!(!turn.tried_activity("compaction"), "and its activities may be tried again");
        use_up(&mut turn);
        turn.plan_converged().unwrap();
        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
        assert!(turn.may_call_model(), "and the final answer");
    }

    #[test]
    fn advance_plan_finalizes_once_steps_are_exhausted() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(2).unwrap();

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
        let (_, text) =
            given(&mut turn).pop().expect("the instruction to finalize is given to the model");
        assert!(text.contains("completed every step"));
        assert!(started.is_none(), "finalization is not a step, so nothing is announced");
    }

    #[tokio::test]
    async fn plan_instructions_are_given_where_they_were_placed_and_not_kept_for_later_turns() {
        use caelumora_model::context::SystemMessages;
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(agent),
            role,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let mut state = AgentChannelState::default();
        state
            .transcript
            .push(Message { sender_id: ActorId::None, ..text(MessageRole::User, "do it") });
        let step = text(MessageRole::Assistant, "step done");
        let answer = text(MessageRole::Assistant, "all done");
        let keeps_system =
            ModelProfile { system_messages: SystemMessages::Anywhere, ..ModelProfile::default() };
        let answer = {
            state.begin_turn(
                Work::new(
                    EventContext::default(),
                    None,
                    SecurityContext::default(),
                    Classification::default(),
                    Default::default(),
                )
                .given_as(keeps_system),
            );
            let mut turn = Turn::under_way(agent, &mut state);
            turn.set_context(
                vec![PromptPart {
                    role: MessageRole::System,
                    text: "Be brief.".to_string(),
                    after: None,
                    classification: None,
                    source: None,
                }],
                Vec::new(),
            );
            turn.set_plan(plan_of(&["only"]));
            turn.advance_plan(0).unwrap();
            let mut internal = step.clone();
            internal.internal = true;
            turn.push_message(internal);
            turn.advance_plan(1).unwrap();
            let roles = given(&mut turn).into_iter().map(|(role, _)| role).collect::<Vec<_>>();
            use MessageRole::{Assistant, System, User};
            assert_eq!(
                roles,
                [System, User, System, Assistant, System],
                "a model that keeps them in place is given system messages"
            );
            let answer = turn.push_message(answer.clone());
            turn.commit(false, &Discard).await;
            answer
        };

        let recorded = seen(&state, agent, &answer).expect("the agent produced it");
        let order = recorded
            .iter()
            .map(|item| match item {
                ContextItem::Prompt { text, .. } => {
                    text.lines().next().unwrap_or_default().to_string()
                }
                ContextItem::Conversation { message } => {
                    message.content.clone().unwrap_or_default()
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                "Be brief.",
                "do it",
                "Step 1 of 1: only",
                "step done",
                "You have completed every step of the plan:"
            ],
            "each instruction where it was given"
        );
        assert_eq!(
            state.transcript.messages().len(),
            3,
            "the instructions are not in the transcript"
        );

        state.begin_turn(
            Work::new(
                EventContext::default(),
                None,
                SecurityContext::default(),
                Classification::default(),
                Default::default(),
            )
            .given_as(keeps_system),
        );
        let mut turn = Turn::under_way(agent, &mut state);
        turn.set_context(Vec::new(), Vec::new());
        assert_eq!(given(&mut turn).len(), 3, "a later turn is not given them");
    }

    #[test]
    fn advance_plan_finalizes_immediately_for_an_empty_plan() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_plan(plan_of(&[]));

        assert!(turn.advance_plan(0).unwrap().is_none());
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
        assert!(turn.plan_converged().unwrap().is_empty(), "no plan, nothing to move on");
        turn.set_plan(plan_of(&["first", "second"]));
        assert!(turn.in_plan_step());

        let kinds =
            |events: Vec<ActorEvent>| events.iter().map(ActorEvent::kind).collect::<Vec<_>>();
        assert_eq!(kinds(turn.plan_converged().unwrap()), ["PlanStepFinished", "PlanStepStarted"]);
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Step(1)));
        assert_eq!(
            kinds(turn.plan_converged().unwrap()),
            ["PlanStepFinished"],
            "the final answer is no step"
        );
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Finalizing));
        assert!(!turn.in_plan_step(), "the final answer is not internal");
        assert!(turn.plan_converged().unwrap().is_empty());
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
        state.transcript.push(Message {
            role: MessageRole::User,
            content: Some("do it".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        });
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        turn.set_context(Vec::new(), Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));
        turn.advance_plan(0).unwrap();
        turn.push_message(message("did the first"));
        turn.advance_plan(1).unwrap();
        turn.push_message(message("working on the second"));
        let at_first = turn
            .situation()
            .agent_channel_state()
            .transcript
            .entries()
            .next()
            .map(|node| node.digest);
        turn.compact(vec![(message("summary"), None)], at_first).expect("compacted");
        assert_eq!(
            given(&mut turn).len(),
            6,
            "nothing of the plan was replaced, so it is not named again: {:?}",
            given(&mut turn)
        );

        // From its last message: the head is where the compaction's part ends.
        let kept_from = turn
            .situation()
            .agent_channel_state()
            .transcript
            .entries()
            .rev()
            .find(|node| node.entry.message().is_some())
            .map(|node| node.digest);
        turn.compact(vec![(message("summary again"), None)], kept_from).expect("compacted");
        let texts: Vec<String> = given(&mut turn).into_iter().map(|(_, text)| text).collect();
        assert_eq!(texts.len(), 3, "the summary, the plan named again, what was kept: {texts:?}");
        assert_eq!(texts[0], "summary again");
        assert!(texts[1].starts_with("You are carrying out a plan: do the thing\n1. [done] first\n2. [under way] second\n\nStep 2 of 2: second"), "{}", texts[1]);
        assert_eq!(texts[2], "working on the second");
    }
}
