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
//! the service its kind uses. Its tool is offered where a file can be
//! published (issue 290): where the turn may use a workspace in its
//! channel, and a storage of content is configured; a call is checked again
//! all the same, and fails and says so. The result is classified as the
//! artifact, which is classified as the workspace.

use crate::manager::{ArtifactError, ArtifactManager};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    arguments, described, tool_text, usable_workspace, ApprovalMode, ToolError, ToolOutput,
    ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_model::prompt_catalogue::PromptTexts;
use caelumora_model::question::Answers;
use caelumora_model::scope::Scope;
use caelumora_model::security::Grant;
use caelumora_model::store::StoreError;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_model::value::name::ScopedName;
use caelumora_runtime::component::{
    config_of_params, ComponentFacets, ComponentKind, ComponentRegistry, ComponentResult,
};
use caelumora_runtime::content::ContentError;
use caelumora_runtime::terminal::WorkspacePath;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// A file published as a version of an artifact (issue 271): its
/// `name`, `version`, `size`, `description` and the `artifact` it is
/// downloaded from.
pub const ARTIFACT_PUBLISH_PRESENTATION: ToolPresentation =
    ToolPresentation::new("artifact_publish");

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
    artifacts: Arc<ArtifactManager>,
    /// What its tool set is for and its tool, as the configuration or the
    /// prompt catalogue words them (issue 57).
    description: String,
    tools: Vec<Tool>,
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
    /// of the turn's workspace through `artifacts`, its texts from `prompts`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for parameters it does not take;
    /// `ComponentError::Prompt` if a text cannot be had.
    pub fn new(
        params: &Value,
        artifacts: Arc<ArtifactManager>,
        prompts: &PromptTexts,
    ) -> ComponentResult<Self> {
        let config: ArtifactToolConfig = config_of_params(params)?;
        let string = ToolParameters::string;
        let tool = Tool {
            name: PUBLISH_ARTIFACT.to_string(),
            description: String::new(),
            parameters: ToolParameters::object(
                [("path", string()), ("name", string()), ("description", string())],
                ["path"],
            ),
            requires_approval: config.requires_approval.required(false),
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: ARTIFACT_PUBLISH_PRESENTATION,
        };
        let tools = vec![described(prompts, NAMESPACE, ARTIFACT_TOOL_KIND, tool, ())?];
        let description = match &config.description {
            Some(description) => description.clone(),
            None => tool_text(prompts, NAMESPACE, ARTIFACT_TOOL_KIND, "description", ())?,
        };
        Ok(Self { artifacts, description, tools })
    }
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
impl ToolProvider for ArtifactTool {
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
        if !self.artifacts.is_configured() {
            return Err("no storage of content is configured".to_string());
        }
        usable_workspace(situation).await?;
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
        let ctx = turn.situation().security_context();
        let channel = (turn.situation().session_id(), turn.situation().channel_id());
        // Into the project its session's results go to (issue 467), as the session is now.
        let session = turn.situation().session_view().await?.ok_or_else(|| {
            ToolError::ExecutionFailed("the turn is in no session to publish in".to_string())
        })?;
        let name = ScopedName::new(name, Scope::ProjectScope { project_id: session.project() })
            .map_err(ArtifactError::from)?;
        let (workspace, terminal) = turn.workspace_terminal(Grant::Read).await?;
        let content = terminal.read(&at).await?;
        let (artifact, number) = self
            .artifacts
            .publish(ctx, channel, content, &workspace.classification, &name, description)
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

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(ARTIFACT_TOOL_KIND)
                .provides::<dyn ToolProvider>()
                .uses::<ArtifactManager>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let artifacts = Arc::clone(context.service::<ArtifactManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ArtifactTool::new(
                &descriptor.params,
                artifacts,
                &texts,
            )?));
            Ok(facets)
        },
    );
}
