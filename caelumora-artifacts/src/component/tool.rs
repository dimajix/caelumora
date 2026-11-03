//! The facet `ToolProvider` of `artifacts` (issue 271): `publish_artifact`, which publishes a
//! file of the session's workspace as a version of an artifact: a fixed copy, kept beyond the
//! workspace, downloaded at the message that published it. Beside it, `publish_file` of
//! `workspace` offers a file as a link to it as it is now (issue 273).
//!
//! The workspace is reached as `workspace` reaches it to read, through the reader of it the
//! situation hands out (`WorkspaceReader`, issues 344, 516); the file read is handed to the
//! `ArtifactPublisher` the plugin binds to the turn's work (issue 516). Its tool is offered where a
//! file can be published (issue 290): where the turn may use a workspace in its channel, and a
//! storage of content is configured; a call is checked again all the same, and fails and says so.
//! The result is classified as the artifact, which is classified as the workspace.

use super::{ArtifactsComponent, ARTIFACTS_KIND};
use crate::extension::{ArtifactError, ArtifactPublisher};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    arguments, described, workspace_reader, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::error::ContentError;
use caelumora_api::terminal::WorkspacePath;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreError;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_domain::value::name::ScopedName;
use serde::Deserialize;
use serde_json::{json, Value};

/// A file published as a version of an artifact (issue 271): its
/// `name`, `version`, `size`, `description` and the `artifact` it is
/// downloaded from.
pub const ARTIFACT_PUBLISH_PRESENTATION: ToolPresentation =
    ToolPresentation::new("artifact_publish");

/// The one tool of the component.
pub const PUBLISH_ARTIFACT: &str = "publish_artifact";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// The tool `publish_artifact`, asked for as `requires_approval` says, its texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
pub(super) fn publish_artifact(
    prompts: &PromptTexts,
    requires_approval: bool,
) -> Result<Tool, PromptError> {
    let string = ToolParameters::string;
    let tool = Tool {
        name: PUBLISH_ARTIFACT.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [("path", string()), ("name", string()), ("description", string())],
            ["path"],
        ),
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: ARTIFACT_PUBLISH_PRESENTATION,
    };
    described(prompts, NAMESPACE, ARTIFACTS_KIND, tool, ())
}

/// What publishing failed of, as the model is told it: what it can correct - a name, a file, a
/// label - is a parameter; the storage failing is not. A file that changed while it was kept is the
/// storage's `Changed`, told with what the model can do about it.
impl From<ArtifactError> for ToolError {
    fn from(error: ArtifactError) -> Self {
        match error {
            ArtifactError::Invalid(_) | ArtifactError::ClassifiedOtherwise(_) => ToolError::InvalidParameter(error.to_string()),
            ArtifactError::Content(ContentError::Changed) => ToolError::InvalidParameter("the file changed while it was published; publish it again once it is written".to_string()),
            ArtifactError::Content(e) => e.into(),
            ArtifactError::StorageGone(e) => e.into(),
            ArtifactError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter("an artifact of that name in this project is someone else's: publish under another name".to_string()),
            ArtifactError::Store(StoreError::PermissionDenied(_)) => ToolError::PermissionDenied("publishing an artifact here".to_string()),
            error => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

#[async_trait]
impl ToolProvider for ArtifactsComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    /// Its tool where a file can be published: where the work may use a
    /// workspace in its channel, and a storage of content is configured
    /// (issues 290, 282).
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        // A work that reaches no workspace is told so first; one that does is given no publisher
        // only where no storage of content is configured.
        workspace_reader(situation).await.map_err(|e| e.to_string())?;
        if !situation.has_writer::<dyn ArtifactPublisher>().await {
            return Err("no storage of content is configured".to_string());
        }
        Ok(self.tools())
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

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
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
        let channel = (turn.situation().session_id(), turn.situation().channel_id());
        // Into the project its session's results go to (issue 467), as the session is now.
        let session = turn.situation().session().await?.ok_or_else(|| {
            ToolError::ExecutionFailed("the turn is in no session to publish in".to_string())
        })?;
        let name = ScopedName::new(name, Scope::ProjectScope { project_id: session.project() })
            .map_err(ArtifactError::from)?;
        let (workspace, terminal) = workspace_reader(&turn.situation()).await?.terminal().await?;
        let content = terminal.read(&at).await?;
        let publisher =
            turn.writer::<dyn ArtifactPublisher>().await.ok_or(ArtifactError::NotConfigured)?;
        let (artifact, number) = publisher
            .publish(channel, content, &workspace.classification, &name, description)
            .await?;
        let version = artifact.version(number).ok_or_else(|| {
            ToolError::ExecutionFailed(format!("version {number} of the artifact is missing"))
        })?;
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
