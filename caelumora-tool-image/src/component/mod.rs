//! The component kind `image_generation` (issues 512, 525, 534): what it is configured with and
//! built of - the image models its agent is given (`ImageModelClient`, of components of the kind
//! `image_model`) -; its tool `generate_image`, which generates an image with one of them, is its
//! facet `ToolProvider` (`tool`).

mod tool;

use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, ToolProvider};
use caelumora_api::component::{ComponentError, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::image::ImageModelClient;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::tool::Tool;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
const IMAGE_GENERATION_KIND: &str = "image_generation";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageGenerationParams {
    /// Whether calls require approval. Left out, they do not: a call changes nothing but the
    /// server's time, and what leaves is its arguments, which `destination` bounds.
    #[serde(default = "approved_by_none")]
    requires_approval: ApprovalMode,
    /// What the servers of its models are cleared for: where the arguments of a call go - and
    /// stay, in a server's history and in the metadata of what it saves. Left out, public.
    #[serde(default)]
    destination: Option<Classification>,
    /// What the tool is for, as its tool set tells it; left out, the catalogue's text.
    #[serde(default)]
    description: Option<String>,
}

impl Params for ImageGenerationParams {}

fn approved_by_none() -> ApprovalMode {
    ApprovalMode::Never
}

/// A component of the kind `image_generation`: the tool `generate_image` over the image models its
/// agent is given.
#[derive(Debug)]
struct ImageGenerationComponent {
    /// Every model offered, in the order of the agent's components; the first is the default.
    models: Vec<Arc<dyn ImageModelClient>>,
    destination: Classification,
    description: String,
    tool: Tool,
}

impl ImageGenerationComponent {
    /// The component configured by `params` over `models`, which are not empty - `register` builds
    /// none without -, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(
        params: ImageGenerationParams,
        models: Vec<Arc<dyn ImageModelClient>>,
        prompts: &PromptTexts,
    ) -> Result<Self, PromptError> {
        let tool =
            tool::generate_image(prompts, params.requires_approval.required(false), &models)?;
        let description = match params.description {
            Some(description) => description,
            None => tool_text(prompts, NAMESPACE, IMAGE_GENERATION_KIND, "description", ())?,
        };
        Ok(Self {
            models,
            destination: params.destination.unwrap_or(Classification::PUBLIC),
            description,
            tool,
        })
    }
}

/// Registers the component kind `image_generation`, which needs the agent's image models - at least
/// one: a component without is refused with `InvalidConfig` - and writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(IMAGE_GENERATION_KIND)
                .params::<ImageGenerationParams>()
                .provides::<dyn ToolProvider>()
                .needs::<dyn ImageModelClient>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let params = ImageGenerationParams::read(&descriptor.params)?;
            let models: Vec<_> = context
                .all::<dyn ImageModelClient>()
                .await?
                .into_iter()
                .map(|model| model.facet)
                .collect();
            if models.is_empty() {
                return Err(ComponentError::InvalidConfig(
                    "the agent has no image model to offer: it needs a component of the kind \
                     'image_model'"
                        .to_string(),
                ));
            }
            let component = ImageGenerationComponent::new(params, models, &prompts)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(component));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::ImageToolLibrary;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::testing::config;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;
    use serde_json::json;

    /// A component without an image model is not built, nor one with a parameter it does not take.
    #[tokio::test]
    async fn a_component_without_an_image_model_or_with_a_parameter_it_does_not_take_is_refused() {
        let kernel =
            TestKernel { libraries: vec![&AgentsLibrary, &ImageToolLibrary], ..Default::default() }
                .build();
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        let refused = Arc::clone(kernel.component_registry())
            .build(
                &config(&[("images", IMAGE_GENERATION_KIND)]),
                &[TypeKey::of::<dyn ToolProvider>()],
                &context,
            )
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("no image model"), "{refused}");
        assert!(
            ImageGenerationParams::read(&json!({ "endpoint": "x" })).is_err(),
            "a parameter it does not take"
        );
    }
}
