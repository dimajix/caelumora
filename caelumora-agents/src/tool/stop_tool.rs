//! The `stop_tool` component: tools with which an agent ends its part in a
//! conversation - `stop`, `bye` and `silence`.

use crate::prompt::NAMESPACE;
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{described, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptError};
use caelumora_model::question::Answers;
use caelumora_model::actor::ActorState;
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

pub const STOP_TOOL_KIND: &str = "stop_tool";

pub const STOP_TOOL_NAME: &str = "stop";
pub const BYE_TOOL_NAME: &str = "bye";
pub const SILENCE_TOOL_NAME: &str = "silence";

#[derive(Debug)]
pub struct StopToolProvider {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl StopToolProvider {
    /// The provider, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: &PromptCatalogue) -> Result<Self, PromptError> {
        let tool = |name: &str| described(prompts, NAMESPACE, STOP_TOOL_KIND, Tool {
            name: name.to_string(),
            description: String::new(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: Default::default(),
        }, ());
        let tools = vec![tool(STOP_TOOL_NAME)?, tool(BYE_TOOL_NAME)?, tool(SILENCE_TOOL_NAME)?];
        Ok(Self { description: tool_text(prompts, NAMESPACE, STOP_TOOL_KIND, "description", ())?, tools })
    }
}

#[async_trait]
impl ToolProvider for StopToolProvider {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(&self, name: &str, _params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        // What it answers tells nothing: classified as the turn (issue 168).
        match name {
            STOP_TOOL_NAME | BYE_TOOL_NAME => {
                info!("Stopping agent {} {}", turn.situation().agent_id(), turn.agent_name());
                turn.request_state(ActorState::Sleeping);
                Ok(ToolOutput::classified(serde_json::json!({ "status": "stopped" }), turn.situation().label()))
            },
            SILENCE_TOOL_NAME => {
                info!("Silencing agent {} {}", turn.situation().agent_id(), turn.agent_name());
                turn.request_state(ActorState::Idle);
                Ok(ToolOutput::classified(serde_json::json!({ "status": "silent" }), turn.situation().label()))
            },
            &_ => {
                Err(ToolError::UnknownTool(name.to_string()))
            }
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(STOP_TOOL_KIND).provides::<dyn ToolProvider>().uses::<PromptCatalogue>(),
        |_descriptor, context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(StopToolProvider::new(context.service::<PromptCatalogue>()?)?));
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

    #[tokio::test]
    async fn test_stop_tool() {
        let provider = StopToolProvider::new(&testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let result = provider.execute(STOP_TOOL_NAME, &json!({}), None, &mut turn).await.unwrap();
        assert_eq!(result.value["status"], "stopped");
        assert_eq!(turn.requested_state(), Some(ActorState::Sleeping), "the turn ends, the agent sleeps");
        provider.execute(SILENCE_TOOL_NAME, &json!({}), None, &mut turn).await.unwrap();
        assert_eq!(turn.requested_state(), Some(ActorState::Idle), "the turn ends, the agent waits");

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, STOP_TOOL_NAME);
        assert_eq!(tools[1].name, BYE_TOOL_NAME);
        assert_eq!(tools[2].name, SILENCE_TOOL_NAME);
    }
}
