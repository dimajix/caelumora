//! The tool `move_file` of `workspace`: a file or directory of the workspace moved or renamed
//! (issue 267), never over something; where it lies now is known as the agent knew it.

use super::super::tool;
use super::super::{changing, of_workspace};
use super::MOVE_FILE;
use super::{changing_alike, knowing, known, last_known, path};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::{TerminalError, WorkspacePath};
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Move {
    from: String,
    to: String,
}

/// Whether moving `from` to `to` renames it: both lie in one directory.
fn renames(from: &WorkspacePath, to: &WorkspacePath) -> bool {
    from.parent() == to.parent()
}

/// A file or directory moved or renamed (issue 267): its `from` and
/// `to`, and whether its result says it was `renamed` - moved within
/// its directory.
pub(super) const FILE_MOVE_PRESENTATION: ToolPresentation = ToolPresentation::new("file_move");

/// `move_file`: moves or renames a file or directory of the workspace, never over something;
/// where it lies now is known as the agent knew it where it was, if it did.
#[derive(Debug)]
pub(super) struct MoveFileTool {
    description: ToolDescription,
}

impl MoveFileTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [("from", ToolParameters::string()), ("to", ToolParameters::string())],
            ["from", "to"],
        );
        Ok(Self {
            description: tool(
                prompts,
                MOVE_FILE,
                parameters,
                requires_approval,
                FILE_MOVE_PRESENTATION,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for MoveFileTool {
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
        let Move { from, to } = arguments(params)?;
        let (from, to) = (path(&from)?, path(&to)?);
        terminal.move_entry(&from, &to).await.map_err(|e| match e {
            TerminalError::InvalidPath(_) if from.is_root() || to.is_root() => {
                ToolError::InvalidParameter(
                    "the workspace itself cannot be moved, nor moved over".to_string(),
                )
            }
            e => ToolError::from(e),
        })?;
        let moved = json!({
            "from": from.to_string(),
            "to": to.to_string(),
            "renamed": renames(&from, &to)
        });
        // Where it lies now, as the agent knew it where it was, if it did.
        let knew = last_known(turn, &workspace, &from).map(|digest| {
            known(turn, (workspace.id, &workspace.classification), &to, Some(digest))
        });
        Ok(of_workspace(knowing(moved, knew), workspace))
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Move { from, to } = arguments(params)?;
        let how = if renames(&path(&from)?, &path(&to)?) { "Rename" } else { "Move" };
        Ok(Some(format!("{how} '{from}' to '{to}' in the workspace")))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        changing_alike(&self.description, &["from", "to"], params)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::knowledge;
    use super::super::MOVE_FILE;
    use super::*;
    use crate::component::testing::component;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn a_file_or_directory_is_renamed_or_moved_but_never_over_something() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({}));
        fs::create_dir_all(world.dir().join("docs")).unwrap();
        fs::write(world.dir().join("docs/plan.md"), "plan").unwrap();
        fs::write(world.dir().join("docs/notes.md"), "notes").unwrap();

        let renamed = tool_named(&tool, MOVE_FILE)
            .execute(&json!({ "from": "docs/plan.md", "to": "docs/plan-v2.md" }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(
            renamed.value,
            json!({ "from": "docs/plan.md", "to": "docs/plan-v2.md", "renamed": true })
        );
        assert_eq!(renamed.classification.as_ref(), Some(&world.workspace.classification));
        let moved = tool_named(&tool, MOVE_FILE)
            .execute(
                &json!({ "from": "docs/plan-v2.md", "to": "archive/2026/plan.md" }),
                None,
                &mut turn,
            )
            .await
            .unwrap()
            .value;
        assert_eq!(moved["renamed"], false);
        assert_eq!(
            fs::read_to_string(world.dir().join("archive/2026/plan.md")).unwrap(),
            "plan",
            "its directories made"
        );

        let over = tool_named(&tool, MOVE_FILE)
            .execute(
                &json!({ "from": "docs/notes.md", "to": "archive/2026/plan.md" }),
                None,
                &mut turn,
            )
            .await;
        assert!(
            matches!(over, Err(ToolError::InvalidParameter(_))),
            "nothing is moved over: {over:?}"
        );
        assert_eq!(fs::read_to_string(world.dir().join("archive/2026/plan.md")).unwrap(), "plan");
        let missing = tool_named(&tool, MOVE_FILE)
            .execute(&json!({ "from": "docs/gone.md", "to": "docs/here.md" }), None, &mut turn)
            .await;
        assert!(matches!(missing, Err(ToolError::InvalidParameter(_))), "{missing:?}");
        let top = tool_named(&tool, MOVE_FILE)
            .execute(&json!({ "from": "", "to": "elsewhere" }), None, &mut turn)
            .await;
        assert!(
            matches!(
                &top,
                Err(ToolError::InvalidParameter(why)) if why.contains("workspace itself")
            ),
            "{top:?}"
        );

        tool_named(&tool, MOVE_FILE)
            .execute(&json!({ "from": "docs", "to": "old/docs" }), None, &mut turn)
            .await
            .expect("a directory with what it holds");
        assert_eq!(fs::read_to_string(world.dir().join("old/docs/notes.md")).unwrap(), "notes");
        drop(turn);
        assert!(knowledge(&state).is_empty(), "a move reads nothing");
    }
}
