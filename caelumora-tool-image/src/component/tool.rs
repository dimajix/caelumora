//! The facet `ToolProvider` of `image_generation` (issues 512, 525): the tool `generate_image`,
//! which generates an image with one of the image models its agent is given, and gives it the
//! agent's model to see.

use super::{ImageGenerationComponent, IMAGE_GENERATION_KIND};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::tool::{
    arguments, described, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::image::{ImageModelClient, ImageRequest};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolContent, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The one tool of the component.
const GENERATE_IMAGE: &str = "generate_image";
/// An image generated: the `prompt` and `negative_prompt` it was described by, its `width` and
/// `height` where the arguments name them, and the `model` and `seed` of its result - the
/// arguments' where it has none.
const IMAGE_PRESENTATION: ToolPresentation = ToolPresentation::new("image");
/// The seeds chosen at random lie below: exact as a number in JSON and in a browser's JavaScript.
const MAX_SEED: u64 = 1 << 53;

/// The arguments of a call of `generate_image`: the model, and what it is asked. What it leaves
/// out, the model says - but the seed, which is chosen at random, so that another call gives
/// another image.
#[derive(Debug, Deserialize)]
struct Call {
    #[serde(default)]
    model: Option<String>,
    prompt: String,
    #[serde(default)]
    negative_prompt: Option<String>,
    #[serde(default)]
    width: Option<u64>,
    #[serde(default)]
    height: Option<u64>,
    #[serde(default = "random_seed")]
    seed: u64,
}

fn random_seed() -> u64 {
    rand::random_range(0..MAX_SEED)
}

/// The tool `generate_image` over `models`, the first the default, its texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
pub(super) fn generate_image(
    prompts: &PromptTexts,
    requires_approval: bool,
    models: &[Arc<dyn ImageModelClient>],
) -> Result<Tool, PromptError> {
    let values = json!({
        "models": models
            .iter()
            .map(|m| json!({ "name": m.name(), "description": m.description() }))
            .collect::<Vec<_>>(),
        "default": models.first().map(|model| model.name()),
    });
    let tool = Tool {
        name: GENERATE_IMAGE.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [
                ("prompt", ToolParameters::string()),
                ("model", ToolParameters::string()),
                ("negative_prompt", ToolParameters::string()),
                ("width", ToolParameters::number()),
                ("height", ToolParameters::number()),
                ("seed", ToolParameters::number()),
            ],
            ["prompt"],
        ),
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: IMAGE_PRESENTATION,
    };
    described(prompts, NAMESPACE, IMAGE_GENERATION_KIND, tool, &values)
}

impl ImageGenerationComponent {
    /// The model named `name`, of those offered; the default if none is named.
    ///
    /// # Errors
    /// `InvalidParameter` for a name of none offered, naming those that are.
    fn model(&self, name: Option<&str>) -> ToolResult<&Arc<dyn ImageModelClient>> {
        let found = match name {
            Some(name) => self.models.iter().find(|model| model.name() == name),
            None => self.models.first(),
        };
        found.ok_or_else(|| {
            let names: Vec<&str> = self.models.iter().map(|model| model.name()).collect();
            ToolError::InvalidParameter(format!(
                "there is no model '{}'; there are: {}",
                name.unwrap_or_default(),
                names.join(", ")
            ))
        })
    }
}

#[async_trait]
impl ToolProvider for ImageGenerationComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![self.tool.clone()]
    }

    fn destination(&self) -> Option<Classification> {
        Some(self.destination.clone())
    }

    /// Generates the image, classified as the turn's label: it follows from the arguments alone.
    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        if name != GENERATE_IMAGE {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let Call { model, prompt, negative_prompt, width, height, seed } = arguments(params)?;
        let model = self.model(model.as_deref())?;
        let request = ImageRequest { prompt, negative_prompt, width, height, seed };
        let initiator = turn.situation().initiator();
        let images = model.generate(initiator, &request).await?;
        let value = json!({
            "model": model.name(),
            "seed": seed,
            "images": images.iter().map(|image| &image.name).collect::<Vec<_>>(),
        });
        let files = images
            .into_iter()
            .map(|image| ToolContent { name: image.name, bytes: image.content })
            .collect();
        Ok(ToolOutput { files, ..ToolOutput::classified(value, turn.situation().label()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ImageGenerationParams;
    use crate::prompts;
    use bytes::Bytes;
    use caelumora_agents::testing::detached_actor_runtime;
    use caelumora_api::component::{ComponentResult, Params};
    use caelumora_api::image::Image;
    use caelumora_api::model::{ModelError, ModelResult};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::work::Work;
    use std::sync::Mutex;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\ndrawn";

    /// An image model of a test: what it was asked, and what it answers.
    #[derive(Debug)]
    struct Painter {
        name: &'static str,
        asked: Mutex<Vec<ImageRequest>>,
        answer: fn(&str) -> ModelResult<Vec<Image>>,
    }

    #[async_trait]
    impl ImageModelClient for Painter {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "paints"
        }

        async fn generate(&self, _: ActorId, request: &ImageRequest) -> ModelResult<Vec<Image>> {
            self.asked.lock().unwrap().push(request.clone());
            (self.answer)(self.name)
        }
    }

    /// The image model `name`, which answers as `answer` says.
    fn painter(name: &'static str, answer: fn(&str) -> ModelResult<Vec<Image>>) -> Arc<Painter> {
        Arc::new(Painter { name, asked: Mutex::default(), answer })
    }

    /// The image `drawn.png`.
    fn drawn(_: &str) -> ModelResult<Vec<Image>> {
        Ok(vec![Image { name: "drawn.png".to_string(), content: Bytes::from_static(PNG) }])
    }

    fn built(params: Value, models: &[Arc<Painter>]) -> ComponentResult<ImageGenerationComponent> {
        let models = models.iter().map(|m| Arc::<Painter>::clone(m) as Arc<dyn ImageModelClient>);
        Ok(ImageGenerationComponent::new(
            ImageGenerationParams::read(&params)?,
            models.collect(),
            &prompts(),
        )?)
    }

    /// What `tool` gives for a call of `generate_image` with `params`, in a turn that reaches no
    /// channel.
    async fn generated(tool: &ImageGenerationComponent, params: Value) -> ToolResult<ToolOutput> {
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            Default::default(),
            None,
            Default::default(),
            Default::default(),
        ));
        let mut turn = Turn::of(&mut state, detached_actor_runtime(AgentId::default()))
            .expect("a turn was begun");
        tool.execute(GENERATE_IMAGE, &params, None, &mut turn).await
    }

    #[tokio::test]
    async fn an_image_is_generated_with_the_model_a_call_names_and_given_the_model() {
        let (fast, slow) = (painter("fast", drawn), painter("slow", drawn));
        let tool = built(json!({}), &[Arc::clone(&fast), Arc::clone(&slow)]).unwrap();
        let params = json!({
            "prompt": "a lighthouse", "model": "slow", "seed": 7, "width": 768,
            "negative_prompt": "fog"
        });
        let output = generated(&tool, params).await.expect("generated");
        let [image] = output.files.as_slice() else { panic!("one image: {:?}", output.files) };
        assert_eq!((image.name.as_str(), image.bytes.as_ref()), ("drawn.png", PNG));
        assert_eq!(output.value, json!({ "model": "slow", "seed": 7, "images": ["drawn.png"] }));
        let asked = slow.asked.lock().unwrap()[0].clone();
        let request = ImageRequest {
            prompt: "a lighthouse".to_string(),
            negative_prompt: Some("fog".to_string()),
            width: Some(768),
            height: None,
            seed: 7,
        };
        assert_eq!(asked, request);
        assert!(fast.asked.lock().unwrap().is_empty(), "the model named alone");

        let output = generated(&tool, json!({ "prompt": "a lighthouse" })).await.unwrap();
        assert_eq!(output.value["model"], "fast", "the first unless named");
        let seed = output.value["seed"].as_u64().expect("the seed taken");
        assert!(seed < MAX_SEED);
        assert_eq!(fast.asked.lock().unwrap()[0].seed, seed, "chosen at random");
    }

    /// What a call names that no model offers, or a request the model does not take, is the
    /// model's to correct; what the model's server failed with is the call's failure.
    #[tokio::test]
    async fn what_a_call_names_is_the_models_to_correct() {
        let unfit = |model: &str| {
            let reason = "width 4096: it takes 512 to 2048".to_string();
            Err(ModelError::Unfit { model: model.to_string(), reason })
        };
        let tool = built(json!({}), &[painter("fast", unfit)]).unwrap();
        let unknown =
            generated(&tool, json!({ "prompt": "x", "model": "flux" })).await.unwrap_err();
        assert!(
            matches!(&unknown, ToolError::InvalidParameter(why) if why.contains("there are: fast")),
            "{unknown}"
        );
        let wide = generated(&tool, json!({ "prompt": "x", "width": 4096 })).await.unwrap_err();
        assert!(
            matches!(&wide, ToolError::InvalidParameter(why) if why.contains("512 to 2048")),
            "{wide}"
        );
        assert!(
            matches!(
                generated(&tool, json!({ "seed": 1 })).await.unwrap_err(),
                ToolError::InvalidParameter(_)
            ),
            "no prompt"
        );

        let failing = |model: &str| {
            Err(ModelError::Backend {
                endpoint: "http://x".to_string(),
                model: model.to_string(),
                message: "out of memory".to_string(),
            })
        };
        let tool = built(json!({}), &[painter("fast", failing)]).unwrap();
        let failed = generated(&tool, json!({ "prompt": "x" })).await.unwrap_err();
        assert!(
            matches!(&failed, ToolError::ExecutionFailed(why) if why.contains("out of memory")),
            "{failed}"
        );
    }

    #[test]
    fn the_tool_offers_the_models_it_is_given_and_asks_nobody() {
        let models = [painter("fast", drawn), painter("slow", drawn)];
        let tool = built(json!({}), &models).unwrap();
        let [generate] = tool.tools().try_into().unwrap();
        assert!(!generate.requires_approval, "asks nobody unless configured");
        assert_eq!(generate.presentation, IMAGE_PRESENTATION);
        assert!(
            generate
                .description
                .ends_with("generated with:\n- fast (the default): paints\n- slow: paints"),
            "one model a line: {}",
            generate.description
        );
        assert_eq!(tool.destination(), Some(Classification::PUBLIC));
        let asked = built(json!({ "requires_approval": true }), &models).unwrap();
        assert!(asked.tools()[0].requires_approval);
    }
}
