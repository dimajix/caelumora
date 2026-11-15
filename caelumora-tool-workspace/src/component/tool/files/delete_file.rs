//! The tool `delete_file` of `workspace`: a file, or an empty directory, of the workspace
//! deleted.

use super::super::tool;
use super::super::{changing, of_workspace};
use super::DELETE_FILE;
use super::{changing_alike, path};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::TerminalError;
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    path: String,
}

/// `delete_file`: deletes a file, or an empty directory, of the workspace.
#[derive(Debug)]
pub(super) struct DeleteFileTool {
    description: ToolDescription,
}

impl DeleteFileTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object([("path", ToolParameters::string())], ["path"]);
        Ok(Self {
            description: tool(
                prompts,
                DELETE_FILE,
                parameters,
                requires_approval,
                ToolPresentation::DEFAULT,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, terminal) = changing(turn).await?;
        let at = path(&arguments::<Delete>(params)?.path)?;
        terminal.delete(&at).await.map_err(|e| match e {
            TerminalError::InvalidPath(_) if at.is_root() => {
                ToolError::InvalidParameter("the workspace itself cannot be deleted".to_string())
            }
            e => ToolError::from(e),
        })?;
        let value = json!({ "path": at.to_string(), "deleted": true });
        Ok(of_workspace(value.into(), workspace))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        Ok(format!("Delete '{}' from the workspace", arguments::<Delete>(params)?.path))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        changing_alike(&self.description, &["path"], params)
    }
}
