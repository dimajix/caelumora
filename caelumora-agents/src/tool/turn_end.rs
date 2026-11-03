//! The `turn_end` component: tools with which an agent ends its part in a
//! conversation - `stop`, `bye` and `silence`.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{described, tool_text, ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::ActorState;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

pub const TURN_END_KIND: &str = "turn_end";

pub const STOP_TOOL_NAME: &str = "stop";
pub const BYE_TOOL_NAME: &str = "bye";
pub const SILENCE_TOOL_NAME: &str = "silence";

#[derive(Debug)]
pub struct TurnEndComponent {
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl TurnEndComponent {
    /// The provider, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of them cannot be had.
    pub fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let tool = |name: &str| {
            described(
                prompts,
                NAMESPACE,
                TURN_END_KIND,
                Tool {
                    name: name.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object(
                        Vec::<(String, ToolParameters)>::new(),
                        Vec::<String>::new(),
                    ),
                    requires_approval: false,
                    visibility: ToolVisibility::SHOWN,
                    classification: None,
                    presentation: Default::default(),
                },
                (),
            )
        };
        let tools = vec![tool(STOP_TOOL_NAME)?, tool(BYE_TOOL_NAME)?, tool(SILENCE_TOOL_NAME)?];
        Ok(Self {
            description: tool_text(prompts, NAMESPACE, TURN_END_KIND, "description", ())?,
            tools,
        })
    }
}

#[async_trait]
impl ToolProvider for TurnEndComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(
        &self,
        name: &str,
        _params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        // What it answers tells nothing: classified as the turn (issue 168).
        match name {
            STOP_TOOL_NAME | BYE_TOOL_NAME => {
                info!("Stopping agent {} {}", turn.situation().agent_id(), turn.agent_name());
                turn.request_state(ActorState::Sleeping);
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "stopped" }),
                    turn.situation().label(),
                ))
            }
            SILENCE_TOOL_NAME => {
                info!("Silencing agent {} {}", turn.situation().agent_id(), turn.agent_name());
                turn.request_state(ActorState::Idle);
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "silent" }),
                    turn.situation().label(),
                ))
            }
            &_ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(TURN_END_KIND).provides::<dyn ToolProvider>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(TurnEndComponent::new(&texts)?));
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

    #[tokio::test]
    async fn test_stop_tool() {
        let provider = TurnEndComponent::new(&testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let result = provider.execute(STOP_TOOL_NAME, &json!({}), None, &mut turn).await.unwrap();
        assert_eq!(result.value["status"], "stopped");
        assert_eq!(
            turn.requested_state(),
            Some(ActorState::Sleeping),
            "the turn ends, the agent sleeps"
        );
        provider.execute(SILENCE_TOOL_NAME, &json!({}), None, &mut turn).await.unwrap();
        assert_eq!(
            turn.requested_state(),
            Some(ActorState::Idle),
            "the turn ends, the agent waits"
        );

        let tools = provider.tools();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, STOP_TOOL_NAME);
        assert_eq!(tools[1].name, BYE_TOOL_NAME);
        assert_eq!(tools[2].name, SILENCE_TOOL_NAME);
    }
}
