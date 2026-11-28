//! The `calculator` component: a tool that evaluates mathematical
//! expressions.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{
    arguments, described, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider,
    ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const CALCULATOR_KIND: &str = "calculator";

pub const CALCULATE_TOOL_NAME: &str = "calculate";

/// The tool `calculate`, which evaluates a mathematical expression.
///
/// What it takes is what `nanoexpr` evaluates in `f64`: the four operations, `%`, powers with
/// `^`, parentheses, the constants `pi` and `e`, and the usual functions. An expression without a
/// finite result - a division by zero, a value outside a function's domain, an overflow - fails
/// as one it cannot read does, and so does one beyond the limits of `nanoexpr`'s evaluator: more
/// than 8 arguments to a function (`max`, `min`), more than 32 levels of nesting.
#[derive(Debug)]
struct CalculateTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for CalculateTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct Expression {
            expression: String,
        }
        let Expression { expression } = arguments(params)?;

        let result = nanoexpr::evaluate::<f64>(&expression).map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to evaluate expression: {}", e))
        })?;

        // Derived from the arguments alone.
        Ok(ToolOutput::of_turn(Value::from(result), turn))
    }
}

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let calculate = CalculateTool {
        description: described(
            prompts,
            NAMESPACE,
            CALCULATOR_KIND,
            ToolDescription {
                name: CALCULATE_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    vec![("expression", ToolParameters::string())],
                    vec!["expression"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            (),
        )?,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, CALCULATOR_KIND, "description", ())?,
        vec![Arc::new(calculate)],
    ))
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(CALCULATOR_KIND).provides::<dyn ToolProvider>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
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
    use std::f64::consts::{E, PI};

    #[tokio::test]
    async fn it_evaluates_each_kind_of_expression_its_description_names() {
        let calculate =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), CALCULATE_TOOL_NAME);
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let cases = vec![
            // The examples of the description of `expression`.
            ("2 + 2", 4.0),
            ("sin(pi/2)", 1.0),
            ("exp(1)", E),
            // The operations, with their precedences.
            ("10 / 4 - 2 * 3", -3.5),
            ("10 % 3", 1.0),
            ("2 * (3 + 4)", 14.0),
            ("2^3^2", 512.0),
            ("-2^2", -4.0),
            ("1.5e3", 1500.0),
            // The constants and functions.
            ("pi", PI),
            ("ln(e)", 1.0),
            ("cos(0) + sqrt(16) + abs(-3)", 8.0),
            ("max(1, 2, 3) + atan2(0, 1)", 3.0),
        ];

        for (expr, expected) in cases {
            let params = json!({ "expression": expr });
            let result = calculate.execute(&params, None, &mut turn).await.unwrap();
            let val = result.value.as_f64().unwrap();
            assert!(
                (val - expected).abs() < 1e-10,
                "Expression {} failed: expected {}, got {}",
                expr,
                expected,
                val
            );
        }
    }

    #[tokio::test]
    async fn an_expression_unreadable_or_without_a_finite_result_fails() {
        let calculate =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), CALCULATE_TOOL_NAME);
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let too_deep = format!("{}1{}", "(".repeat(33), ")".repeat(33));
        let cases = [
            // Unreadable.
            "invalid",
            "2 +",
            "(1 + 2",
            "foo(1)",
            "sqrt(1, 2)",
            // Without a finite result.
            "1 / 0",
            "sqrt(-1)",
            "ln(0)",
            "exp(1000)",
            // Beyond the evaluator's limits.
            "max(1, 2, 3, 4, 5, 6, 7, 8, 9)",
            &too_deep,
        ];
        for expr in cases {
            let params = json!({ "expression": expr });
            let result = calculate.execute(&params, None, &mut turn).await;
            assert!(result.is_err(), "Expression {} should fail, got {:?}", expr, result);
        }
    }
}
