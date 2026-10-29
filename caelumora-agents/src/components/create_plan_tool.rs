//! The `create_plan_tool` component: the `create_plan` tool, and the planning
//! prompt that nudges the model towards it (extended planning).

use crate::prompt::NAMESPACE;
use crate::prompt::{texts_of, writing_for_the_model, ContextError, Prompt, PromptProvider};
use crate::situation::Situation;
use crate::tool::{
    arguments, described, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::plan::{Plan, PlanId, PlanProgress, PlanState, PlanStep};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::question::Answers;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// A plan (issue 263): its `summary` and its `steps`, numbered.
pub const PLAN_PRESENTATION: ToolPresentation = ToolPresentation::new("plan");

pub const CREATE_PLAN_TOOL_KIND: &str = "create_plan_tool";

pub const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

/// The text of the system prompt part contributed by
/// `CreatePlanToolProvider`'s `PromptProvider` impl, nudging the model
/// towards `create_plan` for anything non-trivial, in the prompt catalogue.
/// Composed alongside the personality's own system prompt by
/// `TurnContextProvider` once per turn - it stays for the whole turn
/// (including once a plan is being executed), which is harmless: the far
/// more specific instructions of the plan's steps dominate by then.
const PLANNING: &str = "agents.planning";

/// The `create_plan` tool: available at every point in a turn (not phase-
/// gated) and without approval, as a tool without an effect outside the
/// turn - whoever wants to see plans first sets a rule of a place that asks
/// for it (issue 13), and `describe` shows them the plan. `execute()` parses
/// the plan and either stores it on `Turn` as a fresh plan, or, if one is
/// already in progress, appends the new steps to it (see `execute` below). Also
/// implements `PromptProvider` to contribute `PLANNING_SYSTEM_PROMPT`, which
/// the context provider collects with every other prompt part (issue 99).
#[derive(Debug)]
pub struct CreatePlanToolProvider {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
    /// The texts of its prompt part.
    prompts: PromptTexts,
}

impl CreatePlanToolProvider {
    /// The provider, its texts from `prompts`, told how many steps a plan
    /// has at most; it keeps `prompts` for its prompt part.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: PromptTexts) -> Result<Self, PromptError> {
        // What its texts are told.
        let values = context! { max_steps => Plan::MAX_STEPS };
        let tools = vec![described(
            &prompts,
            NAMESPACE,
            CREATE_PLAN_TOOL_KIND,
            Tool {
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
        )?];
        let description =
            tool_text(&prompts, NAMESPACE, CREATE_PLAN_TOOL_KIND, "description", &values)?;
        Ok(Self { description, tools, prompts })
    }
}

#[async_trait]
impl PromptProvider for CreatePlanToolProvider {
    async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        // A fixed text of the runtime's: nothing confidential (issue 168).
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
impl ToolProvider for CreatePlanToolProvider {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(
        &self,
        _name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
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
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "steps_added" }),
                    turn.situation().label(),
                ))
            }
            None => {
                turn.set_plan(PlanState { plan: incoming, progress: PlanProgress::Step(0) });
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "accepted" }),
                    turn.situation().label(),
                ))
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
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CREATE_PLAN_TOOL_KIND)
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>(),
        ),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let provider = Arc::new(CreatePlanToolProvider::new(texts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<CreatePlanToolProvider>::clone(&provider));
            facets.insert::<dyn PromptProvider>(provider);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use serde_json::json;

    #[test]
    fn create_plan_tool_requires_no_approval() {
        assert!(
            !CreatePlanToolProvider::new(testing::prompts())
                .unwrap()
                .tools()
                .remove(0)
                .requires_approval
        );
    }

    #[test]
    fn create_plan_calls_are_shown_as_a_plan() {
        assert_eq!(
            CreatePlanToolProvider::new(testing::prompts()).unwrap().tools().remove(0).presentation,
            PLAN_PRESENTATION
        );
    }

    #[tokio::test]
    async fn create_plan_tool_execute_stores_the_parsed_plan() {
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

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
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
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
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
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
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, None, &mut turn).await;
        assert!(result.is_ok());

        let plan_state = turn.plan().expect("plan should still be set");
        assert_eq!(plan_state.plan.steps.len(), 2);
        // Redirected back to the newly-added step instead of staying Finalizing.
        assert_eq!(plan_state.progress, PlanProgress::Step(1));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_rejects_malformed_arguments() {
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &json!({}), None, &mut turn).await;
        assert!(
            matches!(result, Err(ToolError::InvalidParameter(_))),
            "the model is asked to correct the call: {result:?}"
        );
        assert!(turn.plan().is_none());
    }

    #[tokio::test]
    async fn a_plan_has_at_most_its_number_of_steps_those_appended_included() {
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);
        let steps = |n: usize| json!({ "summary": "work", "steps": (0..n).map(|i| json!({ "description": format!("step {i}") })).collect::<Vec<_>>() });

        let result = provider
            .execute(CREATE_PLAN_TOOL_NAME, &steps(Plan::MAX_STEPS + 1), None, &mut turn)
            .await;
        assert!(
            matches!(&result, Err(ToolError::InvalidParameter(m)) if m.contains("at most 20")),
            "the model is asked to correct the call: {result:?}"
        );
        assert!(turn.plan().is_none());

        provider
            .execute(CREATE_PLAN_TOOL_NAME, &steps(Plan::MAX_STEPS - 1), None, &mut turn)
            .await
            .expect("one step less than it may have");
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &steps(2), None, &mut turn).await;
        assert!(
            matches!(result, Err(ToolError::InvalidParameter(_))),
            "appended, it would have one too many: {result:?}"
        );
        assert_eq!(
            turn.plan().expect("the plan stays").plan.steps.len(),
            Plan::MAX_STEPS - 1,
            "as it was"
        );
        provider
            .execute(CREATE_PLAN_TOOL_NAME, &steps(1), None, &mut turn)
            .await
            .expect("the last step it may have");
        assert_eq!(turn.plan().unwrap().plan.steps.len(), Plan::MAX_STEPS);
    }

    #[test]
    fn create_plan_tool_describe_renders_summary_and_steps() {
        let provider = CreatePlanToolProvider::new(testing::prompts()).unwrap();
        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Find the root cause" },
            ]
        });

        let description =
            provider.describe(CREATE_PLAN_TOOL_NAME, &params).expect("valid plan should describe");
        assert!(description.contains("Investigate and fix the bug"));
        assert!(description.contains("1. Reproduce the issue"));
        assert!(description.contains("2. Find the root cause"));
    }

    #[tokio::test]
    async fn create_plan_tool_contributes_the_planning_system_prompt() {
        let mut state = AgentChannelState::default();
        let turn = Turn::begun(Default::default(), &mut state);
        let prompt = CreatePlanToolProvider::new(testing::prompts())
            .unwrap()
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
            matches!(parse_plan(&args), Err(ToolError::InvalidParameter(ref m)) if m.contains("steps")),
            "serde names what is missing"
        );
    }
}
