//! The `artifact_tool` component (issue 271): artifacts, for the agents of
//! the personalities that include it - so far `publish_artifact`, which
//! publishes a file of the session's workspace as a version of an artifact:
//! a fixed copy, kept beyond the workspace, downloaded at the message that
//! published it. Beside it, `publish_file` of `terminal_tool` offers a file
//! as a link to it as it is now (issue 273).
//!
//! The workspace is reached as `terminal_tool` reaches it, through the
//! turn's views, and the artifact's manager through the workspace's view:
//! components are given no services when they are made. Without an
//! artifact storage configured, a call fails and says so. The result is
//! classified as the artifact, which is classified as the workspace.

use crate::agent::turn::Turn;
use crate::artifact::ArtifactError;
use crate::component::{config_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::terminal::WorkspacePath;
use crate::tool::{arguments, channel_workspace, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::store::StoreError;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub const ARTIFACT_TOOL_KIND: &str = "artifact_tool";
pub const PUBLISH_ARTIFACT: &str = "publish_artifact";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactToolConfig {
    /// Whether a publication needs approval: unless configured, it does -
    /// it hands a copy of a file to whoever reads the channel, beyond the
    /// workspace. `"unless_read_only"` asks too: it changes what is kept.
    #[serde(default)]
    requires_approval: ApprovalMode,
    /// What the tool set is for, written for a model (issue 194); a text of
    /// its own unless configured.
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug)]
pub struct ArtifactTool {
    config: ArtifactToolConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// What publishing failed of, as the model is told it: what it can correct
/// - a name, a file, a label - is a parameter; the storage failing is not.
fn failed(error: ArtifactError) -> ToolError {
    match error {
        ArtifactError::Invalid(_) | ArtifactError::ClassifiedOtherwise(_) | ArtifactError::TooLarge { .. } | ArtifactError::Changed => ToolError::InvalidParameter(error.to_string()),
        ArtifactError::Terminal(e) => super::terminal_tool::failed(e),
        ArtifactError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter("an artifact of that name in this session is someone else's: publish under another name".to_string()),
        ArtifactError::Store(StoreError::PermissionDenied) => ToolError::PermissionDenied("publishing an artifact here".to_string()),
        error => ToolError::ExecutionFailed(error.to_string()),
    }
}

#[async_trait]
impl ToolProvider for ArtifactTool {
    fn description(&self) -> String {
        self.config.description.clone().unwrap_or_else(|| {
            "Artifacts: results published from the session's workspace as fixed versions, kept beyond the workspace and downloaded where they were published.".to_string()
        })
    }

    fn tools(&self) -> Vec<Tool> {
        let string = ToolParameters::string;
        vec![Tool {
            name: PUBLISH_ARTIFACT.to_string(),
            description: "Publish a file of the session's workspace as an artifact for the users of the channel: a fixed copy of it as it is now, kept beyond the workspace and downloaded at this message. Publishing again under the same name adds a version. Use it for a result to be kept; to point to a file for a look now, use publish_file.".to_string(),
            parameters: ToolParameters::object(
                [
                    ("path", string("The file, relative to the workspace's top")),
                    ("name", string("The artifact's name, at most 100 characters; the file's name unless given")),
                    ("description", string("What it is, in a sentence, for whoever downloads it")),
                ],
                ["path"],
            ),
            requires_approval: self.config.requires_approval.required(false),
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: ToolPresentation::ArtifactPublish,
        }]
    }

    /// What an approver reads: which file is published as what.
    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        if name != PUBLISH_ARTIFACT {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let Publish { path, name, .. } = arguments(params)?;
        Ok(match name {
            Some(name) => format!("Publish '{path}' in the workspace as the artifact '{name}'"),
            None => format!("Publish '{path}' in the workspace as an artifact"),
        })
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name != PUBLISH_ARTIFACT {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let Publish { path, name, description } = arguments(params)?;
        let at = WorkspacePath::parse(&path).map_err(super::terminal_tool::failed)?;
        let name = match name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => at.file_name().unwrap_or_default().to_string(),
        };
        let description = description.as_deref().map(str::trim).filter(|d| !d.is_empty());
        let workspace = channel_workspace(turn).await?;
        let (artifact, number) = workspace.publish_artifact(&at, &name, description).await.map_err(failed)?;
        let version = artifact.version(number).ok_or_else(|| ToolError::ExecutionFailed(format!("version {number} of the artifact is missing")))?;
        let mut result = json!({
            "artifact": artifact.id,
            "version": number,
            "name": artifact.name.name.to_string(),
            "size": version.size,
            "digest": version.digest,
            "path": at.to_string(),
        });
        // The version's, as it was published (issue 23).
        if let Some(description) = &version.description {
            result["description"] = json!(description.to_string());
        }
        Ok(ToolOutput::classified(result, artifact.classification.clone()))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(ARTIFACT_TOOL_KIND).provides::<dyn ToolProvider>(),
        |descriptor, _context| {
            let config: ArtifactToolConfig = config_of_params(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ArtifactTool { config }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;

    fn tool(params: Value) -> ArtifactTool {
        ArtifactTool { config: serde_json::from_value(params).unwrap() }
    }

    #[tokio::test]
    async fn a_file_is_published_as_versions_of_an_artifact_kept_apart_from_the_workspace() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = tool(json!({}));
        std::fs::create_dir_all(world.dir().join("out")).unwrap();
        std::fs::write(world.dir().join("out/report.csv"), "a,b").unwrap();

        let first = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "description": "The figures" }), None, &mut turn).await.unwrap();
        assert_eq!(first.classification.as_ref(), Some(&world.workspace.classification), "classified as the workspace");
        let first = first.value;
        assert_eq!((first["name"].as_str(), first["version"].as_u64(), first["size"].as_u64(), first["description"].as_str()), (Some("report.csv"), Some(1), Some(3), Some("The figures")));
        assert_eq!(first["digest"], caelumora_model::digest::content(b"a,b").to_string());

        std::fs::write(world.dir().join("out/report.csv"), "a,b\n1,2").unwrap();
        let second = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "name": "report.csv" }), None, &mut turn).await.unwrap().value;
        assert_eq!((second["artifact"].clone(), second["version"].as_u64()), (first["artifact"].clone(), Some(2)), "a version of the same artifact");
        let kept = world.artifacts.path().join(first["artifact"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(kept.join("1")).unwrap(), "a,b", "the first as it was");
        assert_eq!(std::fs::read_to_string(kept.join("2")).unwrap(), "a,b\n1,2");
        std::fs::remove_file(world.dir().join("out/report.csv")).unwrap();
        assert!(kept.join("2").is_file(), "kept beyond the file");

        let missing = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv" }), None, &mut turn).await;
        assert!(matches!(missing, Err(ToolError::InvalidParameter(_))), "{missing:?}");
        let directory = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out" }), None, &mut turn).await;
        assert!(matches!(directory, Err(ToolError::InvalidParameter(_))), "{directory:?}");
        std::fs::write(world.dir().join("out/x.csv"), "x").unwrap();
        let long = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/x.csv", "name": "x".repeat(101) }), None, &mut turn).await;
        assert!(matches!(long, Err(ToolError::InvalidParameter(_))), "a name of at most 100 characters: {long:?}");
    }

    #[test]
    fn a_publication_is_approved_as_a_change_and_presented_as_one() {
        let asks = |mode: Value| tool(json!({ "requires_approval": mode })).tools()[0].requires_approval;
        assert!(asks(json!(true)) && asks(json!("unless_read_only")), "it changes what is kept");
        assert!(!asks(json!(false)));
        assert!(tool(json!({})).tools()[0].requires_approval, "unless configured");
        assert_eq!(tool(json!({})).tools()[0].presentation, ToolPresentation::ArtifactPublish);
        assert_eq!(tool(json!({})).describe(PUBLISH_ARTIFACT, &json!({ "path": "out/r.csv", "name": "Report.csv" })).unwrap(), "Publish 'out/r.csv' in the workspace as the artifact 'Report.csv'");
    }
}
