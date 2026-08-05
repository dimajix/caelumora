use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::ActorState;
use caelumora_model::plan::{Plan, PlanId, PlanProgress, PlanState, PlanStep};
use caelumora_model::tool::{Tool, ToolParameters};
use log::info;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;

pub const ECHO_TOOL_PROVIDER_KIND: &str = "echo_tool";
pub const CALCULATOR_TOOL_PROVIDER_KIND: &str = "calculator_tool";
pub const STOP_TOOL_PROVIDER_KIND: &str = "stop_tool";
pub const CREATE_PLAN_TOOL_PROVIDER_KIND: &str = "create_plan_tool";

pub const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

/// Ephemeral system message appended (only to the LLM prompt, never
/// persisted) while a plan-capable agent hasn't committed to a plan yet,
/// nudging it towards `create_plan` for anything non-trivial.
pub const PLANNING_SYSTEM_PROMPT: &str = "Before responding, decide whether this request needs \
research or multiple steps to complete. If it does, gather any information you need using \
the available tools, then call `create_plan` once with a short summary and an ordered list \
of the steps you will take to fulfill the request. The plan will be shown to the user for \
approval before you execute it. For simple requests that need no research and no multiple \
steps, skip `create_plan` entirely and just answer directly.";

/// The `create_plan` tool: available at every point in a turn (not phase-
/// gated) and approved through the exact same generic tool-call approval
/// pipeline (`requires_approval: true`) as any other tool. `execute()` only
/// ever runs once that approval has already been granted, at which point
/// the submission is inherently "approved" - it parses the plan and either
/// stores it on `TurnContext` as a fresh plan, or, if one is already in
/// progress, appends the new steps to it (see `execute` below).
#[derive(Debug)]
pub struct CreatePlanToolProvider;

#[derive(Deserialize)]
struct CreatePlanArgs {
    summary: String,
    steps: Vec<CreatePlanStepArg>,
}

#[derive(Deserialize)]
struct CreatePlanStepArg {
    description: String,
}

pub fn parse_plan(args: &Value) -> Result<Plan> {
    let args: CreatePlanArgs = serde_json::from_value(args.clone())
        .context("Failed to parse create_plan arguments")?;

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
            requires_approval: true,
        }]
    }

    async fn execute(&self, _name: &str, params: &Value, context: &mut TurnContext) -> ToolResult<Value> {
        let incoming = parse_plan(params).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        match context.take_plan() {
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
                context.set_plan(state);
                Ok(serde_json::json!({ "status": "steps_added" }))
            }
            None => {
                context.set_plan(PlanState { plan: incoming, progress: PlanProgress::Step(0) });
                Ok(serde_json::json!({ "status": "accepted" }))
            }
        }
    }

    fn describe(&self, _name: &str, params: &Value) -> ToolResult<String> {
        let plan = parse_plan(params).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
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

#[derive(Debug)]
pub struct StopToolProvider;

#[async_trait]
impl ToolProvider for StopToolProvider {
    fn tools(&self) -> Vec<Tool> {
        let properties: Vec<(String, ToolParameters)> = vec![];
        let required: Vec<String> = vec![];
        vec![
            Tool {
                name: "stop".to_string(),
                description: "Signal that you have finished your task and have no further input.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: true,
            },
            Tool {
                name: "bye".to_string(),
                description: "Signal that you want to say good-bye and stop the conversation.".to_string(),
                parameters: ToolParameters::object(properties.clone(), required.clone()),
                requires_approval: true,
            },
            Tool {
                name: "silence".to_string(),
                description: "Signal that you do not want to respond to the last message and stay silent instead.".to_string(),
                parameters: ToolParameters::object(properties, required),
                requires_approval: true,
            }
        ]
    }

    async fn execute(&self, name: &str, _params: &Value, context: &mut TurnContext) -> ToolResult<Value> {
        match name {
            "stop" | "bye" => {
                info!("Stopping agent {} {}", context.agent_id, context.agent_name);
                context.request_state(ActorState::Sleeping);
                Ok(serde_json::json!({ "status": "stopped" }))
            },
            "silence" => {
                info!("Silencing agent {} {}", context.agent_id, context.agent_name);
                context.request_state(ActorState::Stopping);
                Ok(serde_json::json!({ "status": "silent" }))
            },
            &_ => {
                Err(ToolError::UnknownTool(name.to_string()))
            }
        }
    }
}

#[derive(Debug)]
pub struct EchoToolProvider;

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "echo".to_string(),
            description: "Echoes the input back".to_string(),
            parameters: ToolParameters::object(
                vec![("message", ToolParameters::string("Input message to be echoed back"))],
                vec!["message"],
            ),
            requires_approval: false,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &mut TurnContext) -> ToolResult<Value> {
        if name == "echo" {
            Ok(params.clone())
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

#[derive(Debug)]
pub struct CalculatorToolProvider;

#[async_trait]
impl ToolProvider for CalculatorToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "calculate".to_string(),
            description: "Evaluates a mathematical expression".to_string(),
            parameters: ToolParameters::object(
                vec![("expression", ToolParameters::string("Mathematical expression to evaluate (e.g., '2 + 2', 'sin(pi/2)', 'exp(1)')"))],
                vec!["expression"],
            ),
            requires_approval: false,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _context: &mut TurnContext) -> ToolResult<Value> {
        if name == "calculate" {
            let expression = params["expression"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidParameter("Missing 'expression' parameter".to_string()))?;

            let result = meval::eval_str(expression)
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to evaluate expression: {}", e)))?;

            serde_json::to_value(result).map_err(|e| ToolError::ExecutionFailed(e.to_string()))
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
    use serde_json::json;

    #[tokio::test]
    async fn test_calculator_tool() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let cases = vec![
            ("2 + 2", 4.0),
            ("2 * (3 + 4)", 14.0),
            ("sin(0)", 0.0),
            ("cos(0)", 1.0),
            ("exp(0)", 1.0),
            ("10 / 2", 5.0),
        ];

        for (expr, expected) in cases {
            let params = json!({ "expression": expr });
            let result = provider.execute("calculate", &params, &mut turn_context).await.unwrap();
            let val = result.as_f64().unwrap();
            assert!((val - expected).abs() < 1e-10, "Expression {} failed: expected {}, got {}", expr, expected, val);
        }
    }

    #[tokio::test]
    async fn test_calculator_tool_invalid_expression() {
        let provider = CalculatorToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let params = json!({ "expression": "invalid" });
        let result = provider.execute("calculate", &params, &mut turn_context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stop_tool() {
        let provider = StopToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let result = provider.execute("stop", &json!({}), &mut turn_context).await.unwrap();
        assert_eq!(result["status"], "stopped");

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "stop");
        assert_eq!(tools[1].name, "bye");
        assert_eq!(tools[2].name, "silence");
    }

    #[test]
    fn create_plan_tool_requires_approval_like_any_other_tool() {
        assert!(CreatePlanToolProvider.tools().remove(0).requires_approval);
    }

    #[tokio::test]
    async fn create_plan_tool_execute_stores_the_parsed_plan() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let params = json!({
            "summary": "Investigate and fix the bug",
            "steps": [
                { "description": "Reproduce the issue" },
                { "description": "Apply and verify the fix" },
            ]
        });

        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, &mut turn_context).await;
        assert!(result.is_ok());

        let plan_state = turn_context.plan().expect("plan should be set after execute");
        assert_eq!(plan_state.plan.summary, "Investigate and fix the bug");
        assert_eq!(plan_state.plan.steps.len(), 2);
        assert_eq!(plan_state.progress, PlanProgress::Step(0));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_appends_steps_to_a_plan_mid_step() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        turn_context.set_plan(PlanState {
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
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, &mut turn_context).await;
        assert!(result.is_ok());

        let plan_state = turn_context.plan().expect("plan should still be set");
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
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        turn_context.set_plan(PlanState {
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
        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &params, &mut turn_context).await;
        assert!(result.is_ok());

        let plan_state = turn_context.plan().expect("plan should still be set");
        assert_eq!(plan_state.plan.steps.len(), 2);
        // Redirected back to the newly-added step instead of staying Finalizing.
        assert_eq!(plan_state.progress, PlanProgress::Step(1));
    }

    #[tokio::test]
    async fn create_plan_tool_execute_rejects_malformed_arguments() {
        let provider = CreatePlanToolProvider;
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let result = provider.execute(CREATE_PLAN_TOOL_NAME, &json!({}), &mut turn_context).await;
        assert!(result.is_err());
        assert!(turn_context.plan().is_none());
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

        assert!(parse_plan(&args).is_err());
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ECHO_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(EchoToolProvider));
            Ok(facets)
        }),
    );
    registry.register(
        CALCULATOR_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CalculatorToolProvider));
            Ok(facets)
        }),
    );
    registry.register(
        STOP_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(StopToolProvider));
            Ok(facets)
        }),
    );
    registry.register(
        CREATE_PLAN_TOOL_PROVIDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(CreatePlanToolProvider));
            Ok(facets)
        }),
    );
}
