//! The `create_plan_tool` component: the `create_plan` tool, and the planning
//! prompt that nudges the model towards it (extended planning).

use crate::agent::turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{ContextError, titled, Prompt, PromptProvider};
use crate::agent::work::AgentWork;
use crate::tool::{arguments, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::plan::{Plan, PlanId, PlanProgress, PlanState, PlanStep};
use caelumora_model::confidentiality::Classification;
use caelumora_model::tool::{Tool, ToolPresentation, ToolVisibility, ToolParameters};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// A plan (issue 263): its `summary` and its `steps`, numbered.
pub const PLAN_PRESENTATION: ToolPresentation = ToolPresentation::new("plan");

pub const CREATE_PLAN_TOOL_KIND: &str = "create_plan_tool";

pub const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

/// System prompt contributed by `CreatePlanToolProvider`'s `PromptProvider`
/// impl, nudging the model towards `create_plan` for anything non-trivial.
/// Composed alongside the personality's own system prompt by
/// `TurnContextProvider` once per turn - it stays for the whole turn
/// (including once a plan is being executed), which is harmless: the far
/// more specific `plan_step_instruction`/`plan_finalize_instruction`
/// dominate by then.
const PLANNING_SYSTEM_PROMPT: &str = "Before responding, decide whether this request needs \
research or multiple steps to complete. If it does, gather any information you need using \
the available tools, then call `create_plan` once with a short summary and an ordered list \
of the steps you will take to fulfill the request. For simple requests that need no research and no multiple \
steps, skip `create_plan` entirely and just answer directly.";

/// The `create_plan` tool: available at every point in a turn (not phase-
/// gated) and without approval, as a tool without an effect outside the
/// turn - whoever wants to see plans first sets a rule of a place that asks
/// for it (issue 13), and `describe` shows them the plan. `execute()` parses
/// the plan and either stores it on `Turn` as a fresh plan, or, if one is
/// already in progress, appends the new steps to it (see `execute` below). Also
/// implements `PromptProvider` to contribute `PLANNING_SYSTEM_PROMPT`, which
/// the context provider collects with every other prompt part (issue 99).
#[derive(Debug)]
pub struct CreatePlanToolProvider;

#[async_trait]
impl PromptProvider for CreatePlanToolProvider {
    async fn get_prompt(&self, _work: &AgentWork<'_>) -> Result<Prompt, ContextError> {
        // A fixed text of the runtime's: nothing confidential (issue 168).
        Ok(Prompt::classified(titled("Planning", PLANNING_SYSTEM_PROMPT), Classification::PUBLIC))
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
        steps: args
            .steps
            .into_iter()
            .map(|s| PlanStep { description: s.description })
            .collect(),
    })
}

#[async_trait]
impl ToolProvider for CreatePlanToolProvider {
    fn description(&self) -> String {
        "Plan a larger piece of work in steps before doing it.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: CREATE_PLAN_TOOL_NAME.to_string(),
            description: "Submit your structured multi-step plan for the user's request. Call \
                this once you have gathered enough information (using other tools if needed) to \
                describe how you will address the request. For simple requests that need no \
                research or multiple steps, just answer directly instead of calling this. If a \
                plan is already in progress, calling this again appends the given steps to it \
                instead of replacing it - use that to add more work once you discover it's \
                needed, whether mid-step or after the original steps are done."
                .to_string(),
            parameters: ToolParameters::object(
                [
                    ("summary", ToolParameters::string("One-sentence summary of the overall approach")),
                    (
                        "steps",
                        ToolParameters::array(
                            ToolParameters::object(
                                [("description", ToolParameters::string("What this step will do"))],
                                ["description"],
                            ),
                            "Ordered list of steps",
                        ),
                    ),
                ],
                ["summary", "steps"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: PLAN_PRESENTATION,
        }]
    }

    async fn execute(&self, _name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let incoming = parse_plan(params)?;
        // What it answers tells nothing: classified as the turn (issue 168).
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
                Ok(ToolOutput::classified(serde_json::json!({ "status": "steps_added" }), turn.label()))
            }
            None => {
                turn.set_plan(PlanState { plan: incoming, progress: PlanProgress::Step(0) });
                Ok(ToolOutput::classified(serde_json::json!({ "status": "accepted" }), turn.label()))
            }
        }
    }

    fn describe(&self, _name: &str, params: &Value) -> ToolResult<String> {
        let plan = parse_plan(params)?;
        let steps = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s.description))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!("Proposed plan: {}\n{}", plan.summary, steps))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CREATE_PLAN_TOOL_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CreatePlanToolProvider));
            facets.insert::<dyn PromptProvider>(Arc::new(CreatePlanToolProvider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolError;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    #[test]
    fn create_plan_tool_requires_no_approval() {
        assert!(!CreatePlanToolProvider.tools().remove(0).requires_approval);
    }

    #[test]
    fn create_plan_calls_are_shown_as_a_plan() {
        assert_eq!(CreatePlanToolProvider.tools().remove(0).presentation, PLAN_PRESENTATION);
    }

    #[tokio::test]
    async fn create_plan_tool_execute_stores_the_parsed_plan() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached());

        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Apply and verify the fix" },
            ]
        });

        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should be set after execute");
        assert_eq!(plan_state.plan.summary, "Investigate and fix the bug");
        assert_eq!(plan_state.plan.steps.len(), 2);
        assert_eq!(plan_state.progress, PlanProgress::Step(0));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_appends_steps_to_a_plan_mid_step() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached());

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
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, None, &mut turn).await;
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
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached());

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
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should still be set");
        assert_eq!(plan_state.plan.steps.len(), 2);
        // Redirected back to the newly-added step instead of staying Finalizing.
        assert_eq!(plan_state.progress, PlanProgress::Step(1));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_rejects_malformed_arguments() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached());

        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &json!({}), None, &mut turn).await;
        assert!(matches!(result, Err(ToolError::InvalidParameter(_))), "the model is asked to correct the call: {result:?}");
        assert!(turn.plan().is_none());
    }

    #[test]
    fn create_plan_tool_describe_renders_summary_and_steps() {
        let provider = CreatePlanToolProvider;
        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Find the root cause" },
            ]
        });

        let description = provider.describe(CREATE_PLAN_TOOL_NAME, &params).expect("valid plan should describe");
        assert!(description.contains("Investigate and fix the bug"));
        assert!(description.contains("1. Reproduce the issue"));
        assert!(description.contains("2. Find the root cause"));
    }

    #[tokio::test]
    async fn create_plan_tool_contributes_the_planning_system_prompt() {
        let mut state = caelumora_model::agent::AgentChannelState::default();
        let turn = Turn::begun(Default::default(), String::new(), &mut state, crate::agent::state::AgentStateHandle::detached());
        let prompt = CreatePlanToolProvider.get_prompt(&turn.work()).await.expect("prompt should be available");
        assert_eq!(prompt.text, format!("[Planning]\n{PLANNING_SYSTEM_PROMPT}\n"), "under a title of its own (issue 249)");
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

        assert!(matches!(parse_plan(&args), Err(ToolError::InvalidParameter(ref m)) if m.contains("steps")), "serde names what is missing");
    }
}
