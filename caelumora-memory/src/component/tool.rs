//! The tools of `agent_memory`: `remember`, `update_memory`, `forget` and
//! `reclassify_memory`, each a type of its own (issue 584), translating a call into what
//! `TurnMemory` does; what they answer tells only what the model asked for.

use super::turn::TurnMemory;
use super::{
    AGENT_MEMORY_KIND, FORGET_TOOL_NAME, RECLASSIFY_MEMORY_TOOL_NAME, REMEMBER_TOOL_NAME,
    UPDATE_MEMORY_TOOL_NAME,
};
use crate::memory::MemoryId;
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::components::notes::NOTE_PRESENTATION;
use caelumora_agents::tool::{
    classification_argument, described, id_argument, reclassify_parameters, text_argument,
    tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
pub(super) fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(RememberTool {
            description: described(
                prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                ToolDescription {
                    name: REMEMBER_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object(
                        [("text", ToolParameters::string())],
                        ["text"],
                    ),
                    requires_approval: false,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        }),
        Arc::new(UpdateMemoryTool {
            description: described(
                prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                ToolDescription {
                    name: UPDATE_MEMORY_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object(
                        [("id", ToolParameters::number()), ("text", ToolParameters::string())],
                        ["id", "text"],
                    ),
                    requires_approval: false,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        }),
        Arc::new(ForgetTool {
            description: described(
                prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                ToolDescription {
                    name: FORGET_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object([("id", ToolParameters::number())], ["id"]),
                    requires_approval: false,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        }),
        Arc::new(ReclassifyMemoryTool {
            description: described(
                prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                ToolDescription {
                    name: RECLASSIFY_MEMORY_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: reclassify_parameters(prompts)?,
                    requires_approval: true,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        }),
    ];
    Ok(FixedTools::new(tool_text(prompts, NAMESPACE, AGENT_MEMORY_KIND, "description", ())?, tools))
}

/// `remember(text)`: remembers `text`, taken over when the turn
/// completes.
#[derive(Debug)]
struct RememberTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for RememberTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id = TurnMemory::remember(turn, text_argument(params)?).await?;
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": "remembered", "id": id.0 }), turn))
    }
}

/// `update_memory(id, text)`: changes the text of an entry that may be
/// recalled here.
#[derive(Debug)]
struct UpdateMemoryTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for UpdateMemoryTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: MemoryId = id_argument(params)?;
        let text = text_argument(params)?;
        if TurnMemory::update(turn, id, text).await? {
            Ok(ToolOutput::of_turn(serde_json::json!({ "status": "updated", "id": id.0 }), turn))
        } else {
            Err(ToolError::InvalidParameter(format!("you remember nothing numbered {id} here")))
        }
    }
}

/// `forget(id)`: forgets an entry that may be recalled here.
#[derive(Debug)]
struct ForgetTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for ForgetTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: MemoryId = id_argument(params)?;
        if TurnMemory::forget(turn, id).await? {
            Ok(ToolOutput::of_turn(serde_json::json!({ "status": "forgotten", "id": id.0 }), turn))
        } else {
            Err(ToolError::InvalidParameter(format!("you remember nothing numbered {id} here")))
        }
    }
}

/// `reclassify_memory(id, level, compartments?)`: reclassifies an entry
/// that may be recalled here, as the policy allows.
#[derive(Debug)]
struct ReclassifyMemoryTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for ReclassifyMemoryTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: MemoryId = id_argument(params)?;
        let classification = classification_argument(params)?;
        TurnMemory::reclassify(turn, id, classification.clone()).await?;
        Ok(ToolOutput::of_turn(
            serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }),
            turn,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROMPTS;
    use caelumora_agents::testing::prompts_with;
    use caelumora_agents::tool::ToolProvider;

    #[test]
    fn every_entry_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = tools(&prompts_with(NAMESPACE, PROMPTS)).unwrap().tools();
        assert!(
            tools
                .iter()
                .map(|t| t.description())
                .all(|t| (t.presentation, t.visibility)
                    == (NOTE_PRESENTATION, ToolVisibility::HIDDEN))
        );
    }
}
