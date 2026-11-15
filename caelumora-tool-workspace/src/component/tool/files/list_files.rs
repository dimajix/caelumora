//! The tool `list_files` of `workspace`: what a directory of the workspace holds, at most
//! `MAX_ENTRIES` entries.

use super::super::reading;
use super::super::tool;
use super::LIST_FILES;
use super::{of_file, path};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::FileEntry;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};

/// The most entries one listing gives.
const MAX_ENTRIES: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct At {
    #[serde(default)]
    path: Option<String>,
}

fn entry(entry: &FileEntry) -> Value {
    json!({ "name": entry.name, "is_dir": entry.is_dir, "size": entry.size, "modified": entry.modified })
}

/// A directory listed (issue 265): its `path`, and the `entries` of its
/// result - each `name`, `is_dir`, `size` and `modified` - and whether
/// they are `complete`.
pub(super) const FILE_LIST_PRESENTATION: ToolPresentation = ToolPresentation::new("file_list");

/// `list_files`: what a directory of the workspace holds.
#[derive(Debug)]
pub(super) struct ListFilesTool {
    description: ToolDescription,
}

impl ListFilesTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters =
            ToolParameters::object([("path", ToolParameters::string())], Vec::<&str>::new());
        Ok(Self {
            description: tool(
                prompts,
                LIST_FILES,
                parameters,
                requires_approval,
                FILE_LIST_PRESENTATION,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for ListFilesTool {
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
        let at = path(&arguments::<At>(params)?.path.unwrap_or_default())?;
        let entries = terminal.list(&at).await?;
        let listed: Vec<Value> = entries.iter().take(MAX_ENTRIES).map(entry).collect();
        let value = json!({ "path": at.to_string(), "entries": listed, "complete": entries.len() <= MAX_ENTRIES });
        Ok(of_file(turn, workspace, &at, None, value))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        Ok(format!("List '{}' in the workspace", arguments::<At>(params)?.path.unwrap_or_default()))
    }
}
