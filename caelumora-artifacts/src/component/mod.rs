//! The component kind `artifacts` (issues 271, 534): what it is configured with - whether a
//! publication is asked for, what its tool set is for -; its tool `publish_artifact`, which
//! publishes a file of the session's workspace as a version of an artifact, is its facet
//! `ToolProvider` (`tool`).

mod tool;

pub use tool::{ARTIFACT_PUBLISH_PRESENTATION, PUBLISH_ARTIFACT};

use crate::extension::ArtifactPublisher;
use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, ToolProvider};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents, Params};
use caelumora_api::workspace::WorkspaceReader;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::tool::Tool;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
pub const ARTIFACTS_KIND: &str = "artifacts";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactsParams {
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

impl Params for ArtifactsParams {}

/// A component of the kind `artifacts`: the tool `publish_artifact`.
#[derive(Debug)]
struct ArtifactsComponent {
    /// What its tool set is for and its tool, as the configuration or the
    /// prompt catalogue words them (issue 57).
    description: String,
    tools: Vec<Tool>,
}

impl ArtifactsComponent {
    /// The component configured by `params`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(params: ArtifactsParams, prompts: &PromptTexts) -> Result<Self, PromptError> {
        let tools =
            vec![tool::publish_artifact(prompts, params.requires_approval.required(false))?];
        let description = match params.description {
            Some(description) => description,
            None => tool_text(prompts, NAMESPACE, ARTIFACTS_KIND, "description", ())?,
        };
        Ok(Self { description, tools })
    }
}

/// Registers the component kind `artifacts`, which reads the agent's workspace and publishes into
/// the plugin's artifacts, and writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(ARTIFACTS_KIND)
                .params::<ArtifactsParams>()
                .provides::<dyn ToolProvider>()
                .reads::<dyn WorkspaceReader>()
                .writes::<dyn ArtifactPublisher>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let params = ArtifactsParams::read(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ArtifactsComponent::new(params, &texts)?));
            Ok(facets)
        },
    );
}
