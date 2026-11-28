//! The `planning` component: the `create_plan` tool, and the planning
//! prompt that nudges the model towards it (extended planning).

use crate::prompt::NAMESPACE;
use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use crate::tool::{
    arguments, described, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider,
    ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::plan::{Plan, PlanId, PlanProgress, PlanState, PlanStep};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// A plan (issue 263): its `summary` and its `steps`, numbered.
pub const PLAN_PRESENTATION: ToolPresentation = ToolPresentation::new("plan");

pub const PLANNING_KIND: &str = "planning";

pub const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

/// The text of the prompt part contributed by `PlanningComponent`'s `PromptProvider` impl, nudging
/// the model towards `create_plan` for anything non-trivial, in the prompt catalogue. Composed by
/// `TurnContextProvider` once per turn - in the system prompt, alongside the personality's own,
/// unless its `position` says otherwise -, it stays for the whole turn (including once a plan is
/// being executed), which is harmless: the far more specific instructions of the plan's steps
/// dominate by then.
const PLANNING: &str = "agents.planning";

/// When to use the tool `create_plan` (`CreatePlanTool`): the `PromptProvider` contributing the
/// text `PLANNING`, which the context provider collects with every other prompt part (issue 99).
#[derive(Debug)]
pub struct PlanningComponent {
    /// The texts of its prompt part.
    prompts: PromptTexts,
    /// Where its prompt part goes: in the system prompt unless configured otherwise.
    position: PromptPosition,
}

impl PlanningComponent {
    /// The prompt part at `position`, its text from `prompts`.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { prompts, position }
    }
}

/// The tool `create_plan`: available at every point in a turn (not phase-gated) and without
/// approval, as a tool without an effect outside the turn - whoever wants to see plans first sets
/// a rule of a place that asks for it (issue 13), and `describe` shows them the plan. `execute`
/// parses the plan and either stores it on `Turn` as a fresh plan, or, if one is already in
/// progress, appends the new steps to it.
#[derive(Debug)]
struct CreatePlanTool {
    description: ToolDescription,
}

/// The tools of the component, their texts from `prompts`, told how many steps a plan has at most.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    // What its texts are told.
    let values = context! { max_steps => Plan::MAX_STEPS };
    let create_plan = CreatePlanTool {
        description: described(
            prompts,
            NAMESPACE,
            PLANNING_KIND,
            ToolDescription {
                name: CREATE_PLAN_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [
                        ("summary", ToolParameters::string()),
                        (
                            "steps",
                            ToolParameters::array(ToolParameters::object(
                                [("description", ToolParameters::string())],
                                ["description"],
                            )),
                        ),
                    ],
                    ["summary", "steps"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: PLAN_PRESENTATION,
            },
            &values,
        )?,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, PLANNING_KIND, "description", &values)?,
        vec![Arc::new(create_plan)],
    ))
}

#[async_trait]
impl PromptProvider for PlanningComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // A fixed text of the agents': nothing confidential (issue 168).
        let text =
            self.prompts.render(PLANNING, context! { create_plan => CREATE_PLAN_TOOL_NAME })?;
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

#[derive(Deserialize)]
struct CreatePlanArgs {
    summary: String,
    steps: Vec<CreatePlanStepArg>,
}

#[derive(Deserialize)]
struct CreatePlanStepArg {
    description: String,
}

/// The plan `args` describe. Arguments that do not describe one are the
/// model's to correct - `InvalidParameter`, not a failure of the tool
/// (issue 82) - and serde's message says what is wrong with them.
fn parse_plan(args: &Value) -> ToolResult<Plan> {
    let args: CreatePlanArgs = arguments(args)?;

    Ok(Plan {
        id: PlanId::new(),
        summary: args.summary,
        steps: args.steps.into_iter().map(|s| PlanStep { description: s.description }).collect(),
    })
}

#[async_trait]
impl Tool for CreatePlanTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let incoming = parse_plan(params)?;
        // A plan larger than a plan may be is the model's to correct; the
        // plan in play stays as it was (issue 412).
        let steps = turn.plan().map_or(0, |state| state.plan.steps.len()) + incoming.steps.len();
        if steps > Plan::MAX_STEPS {
            return Err(ToolError::InvalidParameter(format!(
                "the plan would have {steps} steps, but a plan has at most {}: make the steps larger",
                Plan::MAX_STEPS
            )));
        }
        // What it answers tells nothing.
        match turn.take_plan() {
            Some(mut state) => {
                let start = state.plan.steps.len();
                state.plan.summary = incoming.summary;
                state.plan.steps.extend(incoming.steps);
                if state.progress == PlanProgress::Finalizing {
                    // Already told the model to synthesize a final answer -
                    // redirect it to the newly-added work instead of
                    // silently dropping it.
                    state.progress = PlanProgress::Step(start);
                }
                // Otherwise (mid Step(k), k < start): leave progress
                // untouched - the current step continues uninterrupted, and
                // the new steps get picked up once it converges.
                turn.set_plan(state);
                Ok(ToolOutput::of_turn(serde_json::json!({ "status": "steps_added" }), turn))
            }
            None => {
                turn.set_plan(PlanState { plan: incoming, progress: PlanProgress::Step(0) });
                Ok(ToolOutput::of_turn(serde_json::json!({ "status": "accepted" }), turn))
            }
        }
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let plan = parse_plan(params)?;
        let steps = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s.description))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Some(format!("Proposed plan: {}\n{}", plan.summary, steps)))
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(PLANNING_KIND)
                .params::<PositionParams>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position = PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            facets.insert::<dyn PromptProvider>(Arc::new(PlanningComponent::new(position, texts)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    /// The tool `create_plan`, as the component gives it.
    fn create_plan() -> Arc<dyn Tool> {
        testing::tool_named(&tools(&testing::prompts()).unwrap(), CREATE_PLAN_TOOL_NAME)
    }

    #[test]
    fn create_plan_tool_requires_no_approval() {
        assert!(!create_plan().description().requires_approval);
    }

    #[test]
    fn create_plan_calls_are_shown_as_a_plan() {
        assert_eq!(create_plan().description().presentation, PLAN_PRESENTATION);
    }

    #[tokio::test]
    async fn create_plan_tool_execute_stores_the_parsed_plan() {
        let create_plan = create_plan();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Apply and verify the fix" },
            ]
        });

        let result = create_plan.execute(&params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should be set after execute");
        assert_eq!(plan_state.plan.summary, "Investigate and fix the bug");
        assert_eq!(plan_state.plan.steps.len(), 2);
        assert_eq!(plan_state.progress, PlanProgress::Step(0));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_appends_steps_to_a_plan_mid_step() {
        let create_plan = create_plan();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        turn.set_plan(PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "original".to_string(),
                steps: vec![PlanStep { description: "first".to_string() }],
            },
        });

        let params = json!({
            "summary": "updated",
            "steps": [ { "description": "second" } ]
        });
        let result = create_plan.execute(&params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should still be set");
        assert_eq!(plan_state.plan.summary, "updated");
        assert_eq!(plan_state.plan.steps.len(), 2);
        assert_eq!(plan_state.plan.steps[1].description, "second");
        // Mid-step progress is untouched - the current step continues uninterrupted.
        assert_eq!(plan_state.progress, PlanProgress::Step(0));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_resumes_a_finalizing_plan_when_steps_are_added() {
        let create_plan = create_plan();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        turn.set_plan(PlanState {
            progress: PlanProgress::Finalizing,
            plan: Plan {
                id: PlanId::new(),
                summary: "original".to_string(),
                steps: vec![PlanStep { description: "first".to_string() }],
            },
        });

        let params = json!({
            "summary": "updated",
            "steps": [ { "description": "one more thing" } ]
        });
        let result = create_plan.execute(&params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should still be set");
        assert_eq!(plan_state.plan.steps.len(), 2);
        // Redirected back to the newly-added step instead of staying Finalizing.
        assert_eq!(plan_state.progress, PlanProgress::Step(1));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_rejects_malformed_arguments() {
        let create_plan = create_plan();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let result = create_plan.execute(&json!({}), None, &mut turn).await;
        assert!(
            matches!(result, Err(ToolError::InvalidParameter(_))),
            "the model is asked to correct the call: {result:?}"
        );
        assert!(turn.plan().is_none());
    }

    #[tokio::test]
    async fn a_plan_has_at_most_its_number_of_steps_those_appended_included() {
        let create_plan = create_plan();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);
        let steps = |n: usize| json!({ "summary": "work", "steps": (0..n).map(|i| json!({ "description": format!("step {i}") })).collect::<Vec<_>>() });

        let result = create_plan.execute(&steps(Plan::MAX_STEPS + 1), None, &mut turn).await;
        assert!(
            matches!(&result, Err(ToolError::InvalidParameter(m)) if m.contains("at most 20")),
            "the model is asked to correct the call: {result:?}"
        );
        assert!(turn.plan().is_none());

        create_plan
            .execute(&steps(Plan::MAX_STEPS - 1), None, &mut turn)
            .await
            .expect("one step less than it may have");
        let result = create_plan.execute(&steps(2), None, &mut turn).await;
        assert!(
            matches!(result, Err(ToolError::InvalidParameter(_))),
            "appended, it would have one too many: {result:?}"
        );
        assert_eq!(
            turn.plan().expect("the plan stays").plan.steps.len(),
            Plan::MAX_STEPS - 1,
            "as it was"
        );
        create_plan.execute(&steps(1), None, &mut turn).await.expect("the last step it may have");
        assert_eq!(turn.plan().unwrap().plan.steps.len(), Plan::MAX_STEPS);
    }

    #[test]
    fn create_plan_tool_describe_renders_summary_and_steps() {
        let create_plan = create_plan();
        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Find the root cause" },
            ]
        });

        let description = create_plan
            .describe(&params)
            .expect("valid plan should describe")
            .expect("a plan is described");
        assert!(description.contains("Investigate and fix the bug"));
        assert!(description.contains("1. Reproduce the issue"));
        assert!(description.contains("2. Find the root cause"));
    }

    #[tokio::test]
    async fn create_plan_tool_contributes_the_planning_system_prompt() {
        let mut state = AgentChannelState::default();
        let turn = Turn::begun(Default::default(), &mut state);
        let prompt = PlanningComponent::new(PromptPosition::Start, testing::prompts())
            .get_prompt(&turn.situation())
            .await
            .expect("prompt should be available");
        assert_eq!(prompt.text, "[Planning]\nBefore responding, decide whether this request needs \
            research or multiple steps to complete. If it does, gather any information you need using \
            the available tools, then call `create_plan` once with a short summary and an ordered list \
            of the steps you will take to fulfill the request. For simple requests that need no research and no multiple \
            steps, skip `create_plan` entirely and just answer directly.\n", "under a title of its own (issue 249)");
    }

    #[test]
    fn parse_plan_extracts_summary_and_steps() {
        let args = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Find the root cause" },
                { "description": "Apply and verify the fix" },
            ]
        });

        let plan = parse_plan(&args).expect("valid create_plan arguments should parse");
        assert_eq!(plan.summary, "Investigate and fix the bug");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "Reproduce the issue");
        assert_eq!(plan.steps[2].description, "Apply and verify the fix");
    }

    #[test]
    fn parse_plan_rejects_malformed_arguments() {
        let args = json!({ "summary": "missing steps field" });

        assert!(
            matches!(
                parse_plan(&args),
                Err(ToolError::InvalidParameter(ref m)) if m.contains("steps")
            ),
            "serde names what is missing"
        );
    }
}
