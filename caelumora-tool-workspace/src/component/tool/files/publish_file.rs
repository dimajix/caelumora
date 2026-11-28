//! The tool `publish_file` of `workspace`: a file of the workspace offered for download, as a
//! link to it as it is now with its digest (issue 273); it gives nothing away and never asks
//! (issue 275).

use super::super::reading;
use super::super::tool;
use super::PUBLISH_FILE;
use super::{digest, of_file, path};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use serde::Deserialize;
use serde_json::{json, Value};

/// The longest name a file is offered under, in characters.
const MAX_NAME_CHARS: usize = 255;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// A file of the workspace offered for download (issue 273): its
/// `name`, `size`, `description` and `path`, and whether the file is
/// still as it was (`size`, `modified`) or gone.
pub(super) const FILE_PUBLISH_PRESENTATION: ToolPresentation =
    ToolPresentation::new("file_publish");

/// `publish_file`: a file of the workspace offered for download, as a link to it as it is now.
#[derive(Debug)]
pub(super) struct PublishFileTool {
    description: ToolDescription,
}

impl PublishFileTool {
    /// The tool, its texts from `prompts`. It is never asked (issue 275): it changes nothing, and
    /// hands the file to nobody who could not download it already.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [
                ("path", ToolParameters::string()),
                ("name", ToolParameters::string()),
                ("description", ToolParameters::string()),
            ],
            ["path"],
        );
        Ok(Self {
            description: tool(
                prompts,
                PUBLISH_FILE,
                parameters,
                false,
                FILE_PUBLISH_PRESENTATION,
                (),
            )?,
        })
    }
}

#[async_trait]
impl Tool for PublishFileTool {
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
        let Publish { path: at, name, description } = arguments(params)?;
        let at = path(&at)?;
        let found = terminal.stat(&at).await?;
        if found.is_dir {
            return Err(ToolError::InvalidParameter(format!(
                "'{at}' is a directory; only a file can be offered"
            )));
        }
        let name = match name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => at.file_name().unwrap_or_default().to_string(),
        };
        if name.chars().count() > MAX_NAME_CHARS
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
        {
            return Err(ToolError::InvalidParameter(format!("'{name}' is no name to offer a file under: at most {MAX_NAME_CHARS} characters, no slashes")));
        }
        let (size, digest) = digest(terminal.as_ref(), &at).await?;
        let mut result = json!({ "workspace": workspace.id, "path": at.to_string(), "name": name, "size": size, "modified": found.modified, "digest": digest.to_string() });
        if let Some(description) =
            description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty())
        {
            result["description"] = json!(description);
        }
        // Offered as a link: the model knows the place, not what the file holds.
        Ok(of_file(turn, workspace, &at, None, result))
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Publish { path, name, .. } = arguments(params)?;
        Ok(Some(match name {
            Some(name) => format!("Offer '{path}' in the workspace for download as '{name}'"),
            None => format!("Offer '{path}' in the workspace for download"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::knowledge;
    use super::super::PUBLISH_FILE;
    use super::*;
    use crate::component::testing::component;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::digest;
    use caelumora_domain::workspace::Workspace;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn a_file_is_offered_for_download_as_it_is_with_its_digest() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({}));
        fs::create_dir_all(world.dir().join("out")).unwrap();
        fs::write(world.dir().join("out/report.csv"), "a,b\n1,2\n").unwrap();

        let offered = tool_named(&tool, PUBLISH_FILE).execute(&json!({ "path": "out/report.csv", "name": " Report Q3.csv ", "description": "The figures" }), None, &mut turn).await.unwrap();
        let read: Vec<_> = offered.knowledge.iter().map(|k| k.source.clone()).collect();
        let value = offered.value;
        assert_eq!(
            (
                value["path"].as_str(),
                value["name"].as_str(),
                value["size"].as_u64(),
                value["description"].as_str()
            ),
            (Some("out/report.csv"), Some("Report Q3.csv"), Some(8), Some("The figures"))
        );
        let digest = digest::content(b"a,b\n1,2\n");
        assert_eq!(value["digest"], digest.to_string());
        assert_eq!(value["workspace"], json!(world.workspace.id));
        assert!(value["modified"].is_string(), "{value}");
        assert_eq!(offered.classification.as_ref(), Some(&world.workspace.classification));
        let plain = tool_named(&tool, PUBLISH_FILE)
            .execute(&json!({ "path": "out/report.csv" }), None, &mut turn)
            .await
            .unwrap()
            .value;
        assert_eq!(plain["name"], "report.csv", "its file name unless given");
        assert!(plain.get("description").is_none());

        let directory = tool_named(&tool, PUBLISH_FILE)
            .execute(&json!({ "path": "out" }), None, &mut turn)
            .await;
        assert!(
            matches!(
                &directory,
                Err(ToolError::InvalidParameter(why)) if why.contains("directory")
            ),
            "{directory:?}"
        );
        let missing = tool_named(&tool, PUBLISH_FILE)
            .execute(&json!({ "path": "out/gone.csv" }), None, &mut turn)
            .await;
        assert!(matches!(missing, Err(ToolError::InvalidParameter(_))), "{missing:?}");
        let slashed = tool_named(&tool, PUBLISH_FILE)
            .execute(&json!({ "path": "out/report.csv", "name": "../x.csv" }), None, &mut turn)
            .await;
        assert!(matches!(slashed, Err(ToolError::InvalidParameter(_))), "{slashed:?}");
        assert!(world.dir().join("out/report.csv").is_file(), "nothing changed");

        drop(turn);
        assert!(
            matches!(
                &read[..],
                [source]
                    if source.about::<Workspace>().is_some_and(|(_, d)| d.is_none())
                        && source.place() == Some("out/report.csv")
            ),
            "its place, not its content, which the model was not given: {read:?}"
        );
        assert!(knowledge(&state).is_empty(), "recorded by the executor, not by the tool");
    }
}
