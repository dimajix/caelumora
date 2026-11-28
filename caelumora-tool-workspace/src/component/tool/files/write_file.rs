//! The tool `write_file` of `workspace`: a file of the workspace written whole - only as the
//! agent knew it, if it did -, what it wrote given as what the agent knows of the file now.

use super::super::changing;
use super::super::tool;
use super::WRITE_FILE;
use super::{changing_alike, digest, last_known, of_file, path, unchanged, write};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolOutput, ToolResult};
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
struct Write {
    path: String,
    content: String,
}

/// A file written: its `path` and its `content` as text, and how many
/// `bytes` its result says were written.
pub(super) const FILE_WRITE_PRESENTATION: ToolPresentation = ToolPresentation::new("file_write");

/// `write_file`: writes a file of the workspace whole - only as the agent knew it, if it did
/// (`unchanged`) -, giving what it wrote as what the agent knows of the file now.
#[derive(Debug)]
pub(super) struct WriteFileTool {
    description: ToolDescription,
}

impl WriteFileTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [("path", ToolParameters::string()), ("content", ToolParameters::string())],
            ["path", "content"],
        );
        Ok(Self {
            description: tool(
                prompts,
                WRITE_FILE,
                parameters,
                requires_approval,
                FILE_WRITE_PRESENTATION,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for WriteFileTool {
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
        let Write { path: at, content } = arguments(params)?;
        let at = path(&at)?;
        if let Some(knew) = last_known(turn, &workspace, &at) {
            match terminal.stat(&at).await {
                // A file that is gone takes what is written: nothing is lost.
                Err(TerminalError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
                Ok(_) => unchanged(&at, knew, digest(terminal.as_ref(), &at).await?.1)?,
            }
        }
        let (written, digest) = write(terminal.as_ref(), &at, content).await?;
        let value = json!({ "path": at.to_string(), "bytes": written });
        Ok(of_file(turn, workspace, &at, Some(digest), value))
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Write { path, content } = arguments(params)?;
        Ok(Some(format!("Write '{path}' in the workspace ({} bytes):\n{content}", content.len())))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        changing_alike(&self.description, &["path"], params)
    }
}

#[cfg(test)]
mod tests {
    use super::super::WRITE_FILE;
    use crate::component::testing::component;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_agents::tool::ToolError;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_kernel::workspace::testing::{session_with_workspace, MAX_FILE_BYTES};
    use serde_json::json;

    #[tokio::test]
    async fn a_write_beyond_what_the_workspace_takes_is_refused() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({}));
        let too_large = "x".repeat(MAX_FILE_BYTES as usize + 1);
        let refused = tool_named(&tool, WRITE_FILE)
            .execute(&json!({ "path": "big.txt", "content": too_large }), None, &mut turn)
            .await;
        assert!(
            matches!(
                &refused,
                Err(ToolError::InvalidParameter(why)) if why.contains("larger than")
            ),
            "{refused:?}"
        );
        assert!(!world.dir().join("big.txt").exists());
    }
}
