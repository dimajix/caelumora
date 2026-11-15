//! The `sleep` component: a tool that takes its time, for testing what
//! happens while a tool runs - a cancel, a stop, a lowered channel.
//!
//! The tool `sleep` waits `seconds` - the call's, or the component's
//! parameter `seconds` (default 5) - at most `MAX_SECONDS`, and needs no
//! approval unless the parameter `requires_approval` says so.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{
    arguments, described, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider,
    ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

pub const SLEEP_KIND: &str = "sleep";

pub const SLEEP_TOOL_NAME: &str = "sleep";

/// How long `sleep` waits unless told otherwise.
const DEFAULT_SECONDS: f64 = 5.0;
/// The longest `sleep` waits, whatever it is told.
pub const MAX_SECONDS: f64 = 60.0;

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SleepParams {
    #[serde(default = "default_seconds")]
    seconds: f64,
    #[serde(default)]
    requires_approval: bool,
}

fn default_seconds() -> f64 {
    DEFAULT_SECONDS
}

impl Params for SleepParams {
    /// # Errors
    /// `InvalidConfig` if `seconds` is no time `sleep` waits.
    fn check(&self) -> ComponentResult<()> {
        if duration(self.seconds).is_none() {
            return Err(ComponentError::InvalidConfig(format!(
                "parameter 'seconds' must be a number between 0 and {MAX_SECONDS}"
            )));
        }
        Ok(())
    }
}

/// The tool `sleep`, which waits.
#[derive(Debug)]
struct SleepTool {
    description: ToolDescription,
    /// How long a call without `seconds` waits.
    seconds: f64,
}

/// The tools of the component: `sleep`, which waits `seconds` unless told otherwise, and asks for
/// approval if `requires_approval`; their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(
    seconds: f64,
    requires_approval: bool,
    prompts: &PromptTexts,
) -> Result<FixedTools, PromptError> {
    // What its texts are told, written as Rust writes them - `5`, not `5.0`.
    let values =
        context! { seconds => seconds.to_string(), max_seconds => MAX_SECONDS.to_string() };
    let sleep = SleepTool {
        description: described(
            prompts,
            NAMESPACE,
            SLEEP_KIND,
            ToolDescription {
                name: SLEEP_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    vec![("seconds", ToolParameters::number())],
                    Vec::<&str>::new(),
                ),
                requires_approval,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            &values,
        )?,
        seconds,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, SLEEP_KIND, "description", &values)?,
        vec![Arc::new(sleep)],
    ))
}

/// `seconds` if it is a time to wait: finite, not negative, at most
/// `MAX_SECONDS`.
fn duration(seconds: f64) -> Option<Duration> {
    (seconds.is_finite() && (0.0..=MAX_SECONDS).contains(&seconds))
        .then(|| Duration::from_secs_f64(seconds))
}

#[async_trait]
impl Tool for SleepTool {
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
        struct Asked {
            #[serde(default)]
            seconds: Option<f64>,
        }
        let Asked { seconds } = arguments(params)?;
        let seconds = seconds.unwrap_or(self.seconds);
        let wait = duration(seconds).ok_or_else(|| {
            ToolError::InvalidParameter(format!("'seconds' must be between 0 and {MAX_SECONDS}"))
        })?;
        time::sleep(wait).await;
        // Derived from the arguments alone.
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": "slept", "seconds": seconds }), turn))
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(SLEEP_KIND).params::<SleepParams>().provides::<dyn ToolProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let SleepParams { seconds, requires_approval } = SleepParams::read(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(seconds, requires_approval, &texts)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use std::time::Instant;

    #[tokio::test]
    async fn sleep_waits_as_long_as_it_is_told_within_its_limit() {
        let tool =
            testing::tool_named(&tools(0.05, false, &testing::prompts()).unwrap(), SLEEP_TOOL_NAME);
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);

        let started = Instant::now();
        let slept = tool.execute(&Value::Null, None, &mut turn).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50), "the component's default");
        assert_eq!(slept.value["seconds"], 0.05);

        let told =
            tool.execute(&serde_json::json!({ "seconds": 0 }), None, &mut turn).await.unwrap();
        assert_eq!(told.value["seconds"], 0.0, "the call's");
        let too_long = tool
            .execute(&serde_json::json!({ "seconds": MAX_SECONDS + 1.0 }), None, &mut turn)
            .await;
        assert!(matches!(too_long, Err(ToolError::InvalidParameter(_))));
    }
}
