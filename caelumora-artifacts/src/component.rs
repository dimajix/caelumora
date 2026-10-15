//! The `artifact_tool` component (issue 271): artifacts, for the agents of
//! the personalities that include it - so far `publish_artifact`, which
//! publishes a file of the session's workspace as a version of an artifact:
//! a fixed copy, kept beyond the workspace, downloaded at the message that
//! published it. Beside it, `publish_file` of `terminal_tool` offers a file
//! as a link to it as it is now (issue 273).
//!
//! The workspace is reached as `terminal_tool` reaches it, through the
//! turn's channel (`Turn::workspace_terminal`, issue 344), for
//! reading; the file read is handed to the `ArtifactManager` (issue 296),
//! the service its kind uses. Without an artifact storage configured, a call fails and says so. The result is
//! classified as the artifact, which is classified as the workspace.

use crate::manager::{ArtifactError, ArtifactManager};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::security::Grant;
use caelumora_model::store::StoreError;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_agent::turn::Turn;
use caelumora_runtime::component::{config_of_params, ComponentFacets, ComponentKind, ComponentRegistry, ComponentResult};
use caelumora_runtime::terminal::WorkspacePath;
use caelumora_agent::tool::{arguments, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// A file published as a version of an artifact (issue 271): its
/// `name`, `version`, `size`, `description` and the `artifact` it is
/// downloaded from.
pub const ARTIFACT_PUBLISH_PRESENTATION: ToolPresentation = ToolPresentation::new("artifact_publish");

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
    artifacts: Arc<ArtifactManager>,
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

impl ArtifactTool {
    /// The tool of a component configured with `params`, publishing a file
    /// of the turn's workspace through `artifacts`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for parameters it does not take.
    pub fn new(params: &Value, artifacts: Arc<ArtifactManager>) -> ComponentResult<Self> {
        Ok(Self { config: config_of_params(params)?, artifacts })
    }
}

/// What publishing failed of, as the model is told it: what it can correct
/// - a name, a file, a label - is a parameter; the storage failing is not.
impl From<ArtifactError> for ToolError {
    fn from(error: ArtifactError) -> Self {
        match error {
            ArtifactError::Invalid(_) | ArtifactError::ClassifiedOtherwise(_) | ArtifactError::TooLarge { .. } | ArtifactError::Changed => ToolError::InvalidParameter(error.to_string()),
            ArtifactError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter("an artifact of that name in this session is someone else's: publish under another name".to_string()),
            ArtifactError::Store(StoreError::PermissionDenied(_)) => ToolError::PermissionDenied("publishing an artifact here".to_string()),
            error => ToolError::ExecutionFailed(error.to_string()),
        }
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
            presentation: ARTIFACT_PUBLISH_PRESENTATION,
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
        let at = WorkspacePath::parse(&path)?;
        let name = match name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => at.file_name().unwrap_or_default().to_string(),
        };
        let description = description.as_deref().map(str::trim).filter(|d| !d.is_empty());
        let (ctx, session, channel) = (turn.security_context(), turn.session_id(), turn.channel_id());
        let (workspace, terminal) = turn.workspace_terminal(Grant::Read).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let content = terminal.read(&at).await?;
        let (artifact, number) = self.artifacts.publish(ctx, content, &workspace.classification, &name, description, session, channel).await?;
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
        ComponentKind::new(ARTIFACT_TOOL_KIND).provides::<dyn ToolProvider>().uses::<ArtifactManager>(),
        |descriptor, context| {
            let artifacts = Arc::clone(context.service::<ArtifactManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ArtifactTool::new(&descriptor.params, artifacts)?));
            Ok(facets)
        },
    );
}
