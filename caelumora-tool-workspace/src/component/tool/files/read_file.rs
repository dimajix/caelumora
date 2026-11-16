//! The tool `read_file` of `workspace`: a file of the workspace as text, a page at a time of at
//! most what a result holds, given with its digest as what the agent knows of it.

use super::super::reading;
use super::super::tool;
use super::READ_FILE;
use super::{of_file, path, text};
use crate::component::WorkspaceParams;
use async_trait::async_trait;
use caelumora_agents::tool::page_parameters;
use caelumora_agents::tool::{arguments, page_of, TextPosition, Tool, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    start_offset: Option<usize>,
    #[serde(default)]
    line_count: Option<usize>,
}

/// A file read (issue 251): its `path`, and the `content` of its result
/// as text, with the lines it covers - `start_line`, `start_offset`,
/// `end_line`, `total_lines` - and where to read on (`next`).
pub(super) const FILE_READ_PRESENTATION: ToolPresentation = ToolPresentation::new("file_read");

/// `read_file`: a file of the workspace as text, a page at a time of at most what a result holds.
#[derive(Debug)]
pub(super) struct ReadFileTool {
    description: ToolDescription,
    params: Arc<WorkspaceParams>,
    /// What its results say of where to read on.
    prompts: PromptTexts,
}

impl ReadFileTool {
    /// The tool, giving a file a page at a time as `params` bound it and words it, asking for
    /// approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(
        prompts: &PromptTexts,
        params: &Arc<WorkspaceParams>,
        requires_approval: bool,
    ) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [("path", ToolParameters::string())].into_iter().chain(page_parameters()),
            ["path"],
        );
        let description =
            tool(prompts, READ_FILE, parameters, requires_approval, FILE_READ_PRESENTATION, ())?;
        Ok(Self { description, params: Arc::clone(params), prompts: prompts.clone() })
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, terminal) = reading(turn).await?;
        let Read { path: at, start_line, start_offset, line_count } = arguments(params)?;
        let at = path(&at)?;
        let (text, digest) = text(terminal.as_ref(), &at).await?;
        let mut result = page_of(
            &text,
            &format!("'{at}'"),
            TextPosition::asked(start_line, start_offset),
            line_count,
            self.params.max_output_bytes,
        )?
        .value(&self.prompts);
        result["path"] = json!(at.to_string());
        Ok(of_file(turn, workspace, &at, Some(digest), result))
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        Ok(Some(format!("Read '{}' in the workspace", arguments::<Read>(params)?.path)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::READ_FILE;
    use crate::component::testing::component;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn a_line_longer_than_a_result_is_given_in_parts_of_it_under_its_own_number() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({ "max_output_bytes": 10 }));
        fs::write(world.dir().join("long.txt"), "first\n123456789012345\n").unwrap();

        let args = json!({ "path": "long.txt", "start_line": 2 });
        let cut = tool_named(&tool, READ_FILE).execute(&args, None, &mut turn).await.unwrap().value;
        assert_eq!(cut["content"], "1234567890");
        assert_eq!((cut["start_line"].as_u64(), cut["end_line"].as_u64()), (Some(2), Some(2)));
        let next = cut["next"].as_str().unwrap();
        assert!(next.contains("start_line 2") && next.contains("start_offset 10"), "{cut}");
        let args = json!({ "path": "long.txt", "start_line": 2, "start_offset": 10 });
        let rest =
            tool_named(&tool, READ_FILE).execute(&args, None, &mut turn).await.unwrap().value;
        assert_eq!(rest["content"], "12345\n");
        assert_eq!(rest["start_offset"], 10);
        assert!(rest.get("next").is_none(), "{rest}");
    }
}
