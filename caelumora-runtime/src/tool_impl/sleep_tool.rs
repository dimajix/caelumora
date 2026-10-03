//! The `sleep_tool` component: a tool that takes its time, for testing what
//! happens while a tool runs - a cancel, a stop, a lowered channel.
//!
//! The tool `sleep` waits `seconds` - the call's, or the component's
//! parameter `seconds` (default 5) - at most `MAX_SECONDS`, and needs no
//! approval unless the parameter `requires_approval` says so.

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

pub const SLEEP_TOOL_KIND: &str = "sleep_tool";

pub const SLEEP_TOOL_NAME: &str = "sleep";

/// How long `sleep` waits unless told otherwise.
const DEFAULT_SECONDS: f64 = 5.0;
/// The longest `sleep` waits, whatever it is told.
pub const MAX_SECONDS: f64 = 60.0;

#[derive(Debug)]
pub struct SleepToolProvider {
    /// How long a call without `seconds` waits.
    pub seconds: f64,
    pub requires_approval: bool,
}

impl Default for SleepToolProvider {
    fn default() -> Self {
        Self { seconds: DEFAULT_SECONDS, requires_approval: false }
    }
}

/// `seconds` if it is a time to wait: finite, not negative, at most
/// `MAX_SECONDS`.
fn duration(seconds: f64) -> Option<Duration> {
    (seconds.is_finite() && (0.0..=MAX_SECONDS).contains(&seconds)).then(|| Duration::from_secs_f64(seconds))
}

#[async_trait]
impl ToolProvider for SleepToolProvider {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: SLEEP_TOOL_NAME.to_string(),
            description: format!("Waits the given number of seconds, at most {MAX_SECONDS}, and then says so. For testing."),
            parameters: ToolParameters::object(
                vec![("seconds", ToolParameters::number(format!("How long to wait; {} unless given", self.seconds)))],
                Vec::<&str>::new(),
            ),
            requires_approval: self.requires_approval,
            visibility: ToolVisibility::SHOWN,
            classification: None,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name != SLEEP_TOOL_NAME {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        #[derive(Deserialize)]
        struct Sleep { #[serde(default)] seconds: Option<f64> }
        let Sleep { seconds } = arguments(params)?;
        let seconds = seconds.unwrap_or(self.seconds);
        let wait = duration(seconds)
            .ok_or_else(|| ToolError::InvalidParameter(format!("'seconds' must be between 0 and {MAX_SECONDS}")))?;
        tokio::time::sleep(wait).await;
        // Derived from the arguments alone: classified as the turn (issue 168).
        Ok(ToolOutput::classified(serde_json::json!({ "status": "slept", "seconds": seconds }), turn.label()))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(SLEEP_TOOL_KIND).provides::<dyn ToolProvider>(),
        |descriptor, _context| {
            let seconds = match descriptor.params.get("seconds") {
                None => DEFAULT_SECONDS,
                Some(value) => value.as_f64().filter(|s| duration(*s).is_some())
                    .ok_or_else(|| ComponentError::InvalidConfig(format!("parameter 'seconds' must be a number between 0 and {MAX_SECONDS}")))?,
            };
            let requires_approval = match descriptor.params.get("requires_approval") {
                None => false,
                Some(value) => value.as_bool()
                    .ok_or_else(|| ComponentError::InvalidConfig("parameter 'requires_approval' must be true or false".to_string()))?,
            };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(SleepToolProvider { seconds, requires_approval }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId};

    #[tokio::test]
    async fn sleep_waits_as_long_as_it_is_told_within_its_limit() {
        let tool = SleepToolProvider { seconds: 0.05, requires_approval: false };
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached());

        let started = std::time::Instant::now();
        let slept = tool.execute(SLEEP_TOOL_NAME, &Value::Null, &mut turn).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50), "the component's default");
        assert_eq!(slept.value["seconds"], 0.05);

        let told = tool.execute(SLEEP_TOOL_NAME, &serde_json::json!({ "seconds": 0 }), &mut turn).await.unwrap();
        assert_eq!(told.value["seconds"], 0.0, "the call's");
        let too_long = tool.execute(SLEEP_TOOL_NAME, &serde_json::json!({ "seconds": MAX_SECONDS + 1.0 }), &mut turn).await;
        assert!(matches!(too_long, Err(ToolError::InvalidParameter(_))));
    }
}
