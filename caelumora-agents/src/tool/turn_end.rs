//! The `turn_end` component: tools with which an agent ends its part in a
//! conversation - `stop`, `bye` and `silence`.

use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{described, tool_text, FixedTools, Tool, ToolOutput, ToolProvider, ToolResult};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::ActorState;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

pub const TURN_END_KIND: &str = "turn_end";

pub const STOP_TOOL_NAME: &str = "stop";
pub const BYE_TOOL_NAME: &str = "bye";
pub const SILENCE_TOOL_NAME: &str = "silence";

/// A tool that ends the turn with the current tool step (`stop`, `bye`, `silence`): the agent is
/// left in `state`, and the model told `status`. What it answers tells nothing: classified as the
/// turn (issue 168).
#[derive(Debug)]
struct TurnEndTool {
    description: ToolDescription,
    state: ActorState,
    status: &'static str,
    /// What the log says the agent is made to do: "Stopping", "Silencing".
    logged: &'static str,
}

#[async_trait]
impl Tool for TurnEndTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        _params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        info!("{} agent {} {}", self.logged, turn.situation().agent_id(), turn.agent_name());
        turn.request_state(self.state);
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": self.status }), turn))
    }
}

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let tool = |name: &str, state, status, logged| -> Result<Arc<dyn Tool>, PromptError> {
        let description = described(
            prompts,
            NAMESPACE,
            TURN_END_KIND,
            ToolDescription {
                name: name.to_string(),
                description: String::new(),
                parameters: ToolParameters::none(),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            (),
        )?;
        Ok(Arc::new(TurnEndTool { description, state, status, logged }))
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, TURN_END_KIND, "description", ())?,
        vec![
            tool(STOP_TOOL_NAME, ActorState::Sleeping, "stopped", "Stopping")?,
            tool(BYE_TOOL_NAME, ActorState::Sleeping, "stopped", "Stopping")?,
            tool(SILENCE_TOOL_NAME, ActorState::Idle, "silent", "Silencing")?,
        ],
    ))
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(TURN_END_KIND).provides::<dyn ToolProvider>()),
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

    #[tokio::test]
    async fn test_stop_tool() {
        let provider = tools(&testing::prompts()).unwrap();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut agent_channel_state);

        let result = testing::tool_named(&provider, STOP_TOOL_NAME)
            .execute(&json!({}), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(result.value["status"], "stopped");
        assert_eq!(
            turn.requested_state(),
            Some(ActorState::Sleeping),
            "the turn ends, the agent sleeps"
        );
        testing::tool_named(&provider, SILENCE_TOOL_NAME)
            .execute(&json!({}), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(
            turn.requested_state(),
            Some(ActorState::Idle),
            "the turn ends, the agent waits"
        );

        let names: Vec<String> =
            provider.tools().iter().map(|t| t.description().name.clone()).collect();
        assert_eq!(names, [STOP_TOOL_NAME, BYE_TOOL_NAME, SILENCE_TOOL_NAME]);
    }
}
