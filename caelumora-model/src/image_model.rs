//! The kind `image_model` (issue 525): one of the image models the platform offers, as an agent is
//! given it - its client, which is built for this agent alone. An agent may have several; a tool
//! that generates images offers those it is given.
//!
//! Parameters: `name`, the model of `plugins.model.image` - left out, the one there is. An unknown
//! parameter is refused, so that a misspelt one does not silently fall back to its default.

use crate::manager::ModelManager;
use caelumora_api::component::{ComponentFacets, ComponentKind, Params};
use caelumora_api::image::ImageModelClient;
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::kernel::Acting;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of an image model of an agent.
const IMAGE_MODEL_KIND: &str = "image_model";

/// The parameters of an `image_model` component.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageModelParams {
    #[serde(default)]
    name: Option<String>,
}

impl Params for ImageModelParams {}

/// Registers the kind `image_model` with `registry`, which provides an `ImageModelClient`.
pub(crate) fn register_all(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(IMAGE_MODEL_KIND)
            .params::<ImageModelParams>()
            .provides::<dyn ImageModelClient>()
            .uses::<ModelManager>()
            .uses::<Acting>(),
        |descriptor, context| {
            let params = ImageModelParams::read(&descriptor.params)?;
            // For the agent its components are built for, in its name at each call.
            let acting = Acting::clone(context.service::<Acting>()?);
            let models = context.service::<ModelManager>()?;
            let client = models.image_client_for(acting, params.name.as_deref())?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ImageModelClient>(Arc::new(client));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::IMAGE;
    use crate::testing::{configuring, kernel_of, DRAWING};
    use caelumora_api::component::{ComponentContainer, ComponentResult};
    use caelumora_api::image::ImageRequest;
    use caelumora_api::model::ModelError;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_security::SecurityContext;
    use serde_json::{json, Value};

    /// The kernel of a test that offers the image models `models`, each drawing with its
    /// parameters.
    fn offering(models: Value) -> Kernel {
        kernel_of(configuring(json!({ IMAGE: models }))).unwrap()
    }

    /// The components of `kernel` built of the kind `image_model`, one for each of `params`, as a
    /// tool that needs them asks for them.
    async fn models_of(kernel: &Kernel, params: &[Value]) -> ComponentResult<ComponentContainer> {
        let nodes = params
            .iter()
            .enumerate()
            .map(|(at, params)| ComponentDescriptor {
                params: params.clone(),
                ..ComponentDescriptor::new(&format!("image-{at}"), IMAGE_MODEL_KIND)
            })
            .collect();
        let config = ComponentConfig { component_nodes: nodes };
        let facets = [TypeKey::of::<dyn ImageModelClient>()];
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        Arc::clone(kernel.component_registry()).build(&config, &facets, &context).await
    }

    fn request(width: Option<u64>) -> ImageRequest {
        ImageRequest {
            prompt: "a lighthouse".to_string(),
            negative_prompt: None,
            width,
            height: None,
            seed: 7,
        }
    }

    /// Issue 525: an agent's image model is the platform's image model its component names, by
    /// that name and as it describes itself; what it generates, and what it does not take, are the
    /// model's.
    #[tokio::test]
    async fn an_image_model_is_the_platform_s_model_its_component_names() {
        let kernel = offering(json!({
            "fast": { "type": DRAWING, "description": "fast", "max_width": 1024 },
            "slow": { "type": DRAWING, "description": "slow" }
        }));
        let container =
            models_of(&kernel, &[json!({ "name": "slow" }), json!({ "name": "fast" })]).await;
        let clients = container.unwrap().get_all::<dyn ImageModelClient>(None).unwrap();
        let described: Vec<_> =
            clients.iter().map(|c| (c.facet.name(), c.facet.description())).collect();
        assert_eq!(described, [("slow", "slow"), ("fast", "fast")], "in the order configured");

        let fast = &clients[1].facet;
        let images = fast.generate(ActorId::None, &request(Some(512))).await.unwrap();
        let [image] = images.as_slice() else { panic!("one image: {images:?}") };
        assert_eq!(
            (image.name.as_str(), image.content.as_ref()),
            ("fast.png", &b"a lighthouse 7"[..])
        );
        let unfit = fast.generate(ActorId::None, &request(Some(2048))).await.unwrap_err();
        assert!(matches!(unfit, ModelError::Unfit { ref model, .. } if model == "fast"), "{unfit}");
    }

    /// Issue 525: a component that names no image model is given the one there is; one that names
    /// a model not configured, or none among several, or where none is configured, is not built.
    #[tokio::test]
    async fn an_image_model_not_named_is_the_one_there_is() {
        let one = offering(json!({ "only": { "type": DRAWING, "description": "the one" } }));
        let container = models_of(&one, &[json!({})]).await.unwrap();
        let clients = container.get_all::<dyn ImageModelClient>(None).unwrap();
        assert_eq!(clients[0].facet.name(), "only");
        let refused = models_of(&one, &[json!({ "name": "other" })]).await.unwrap_err();
        let refused = refused.to_string();
        assert!(refused.contains("'name' names 'other', which is not configured"), "{refused}");

        let two = offering(json!({
            "a": { "type": DRAWING, "description": "a" },
            "b": { "type": DRAWING, "description": "b" }
        }));
        let refused = models_of(&two, &[json!({})]).await.unwrap_err().to_string();
        assert!(refused.contains("has to name one where there are several"), "{refused}");

        let none = offering(json!({}));
        let refused = models_of(&none, &[json!({})]).await.unwrap_err().to_string();
        assert!(refused.contains("no image model is configured"), "{refused}");
        let misspelt = models_of(&one, &[json!({ "nmae": "only" })]).await.unwrap_err();
        assert!(misspelt.to_string().contains("'nmae'"), "{misspelt}");
    }
}
