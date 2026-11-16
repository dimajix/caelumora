//! The tool `replace_in_file` of `workspace`: a text replaced in a file of the workspace - once,
//! unless every occurrence is to be replaced -, only as the agent knew the file, if it did.

use super::super::changing;
use super::super::tool;
use super::REPLACE_IN_FILE;
use super::{changing_alike, last_known, of_file, path, text, unchanged, write};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::approval::ToolMatcher;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replace {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

/// A replacement in a file: its `path`, its `old_text` and `new_text`
/// as text, whether it replaces every occurrence (`replace_all`), and
/// how many `replacements` its result counts.
pub(super) const FILE_EDIT_PRESENTATION: ToolPresentation = ToolPresentation::new("file_edit");

/// `replace_in_file`: replaces a text in a file of the workspace - once, unless every occurrence
/// is to be replaced -, only as the agent knew the file, if it did (`unchanged`).
#[derive(Debug)]
pub(super) struct ReplaceInFileTool {
    description: ToolDescription,
}

impl ReplaceInFileTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [
                ("path", ToolParameters::string()),
                ("old_text", ToolParameters::string()),
                ("new_text", ToolParameters::string()),
                ("replace_all", ToolParameters::boolean()),
            ],
            ["path", "old_text", "new_text"],
        );
        Ok(Self {
            description: tool(
                prompts,
                REPLACE_IN_FILE,
                parameters,
                requires_approval,
                FILE_EDIT_PRESENTATION,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for ReplaceInFileTool {
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
        let Replace { path: at, old_text, new_text, replace_all } = arguments(params)?;
        if old_text.is_empty() {
            return Err(ToolError::InvalidParameter("'old_text' must not be empty".to_string()));
        }
        let at = path(&at)?;
        let (text, now) = text(terminal.as_ref(), &at).await?;
        if let Some(knew) = last_known(turn, &workspace, &at) {
            unchanged(&at, knew, now)?;
        }
        let found = text.matches(old_text.as_str()).count();
        let replaced = match (found, replace_all) {
            (0, _) => return Err(ToolError::InvalidParameter(format!("'old_text' does not occur in '{at}'"))),
            (1, _) | (_, true) => text.replace(old_text.as_str(), &new_text),
            (many, false) => return Err(ToolError::InvalidParameter(format!("'old_text' occurs {many} times in '{at}': give more of what surrounds it, or set replace_all"))),
        };
        let (_, digest) = write(terminal.as_ref(), &at, replaced).await?;
        let value = json!({ "path": at.to_string(), "replacements": found });
        Ok(of_file(turn, workspace, &at, Some(digest), value))
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Replace { path, old_text, new_text, replace_all } = arguments(params)?;
        let how = if replace_all { "every occurrence" } else { "once" };
        Ok(Some(format!(
            "Replace in '{path}' in the workspace, {how}:\n--- old\n{old_text}\n+++ new\n{new_text}"
        )))
    }

    fn matcher(&self, params: &Value) -> ToolMatcher {
        changing_alike(&self.description, &["path"], params)
    }
}
