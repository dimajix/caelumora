//! The component kind `comfyui_tool` (issue 512): the tool `generate_image`, which generates an
//! image with one of the workflows the component names on one ComfyUI server, and gives it the
//! model to see.

use crate::client::{ComfyUiClient, ComfyUiError};
use crate::workflow::{Arguments, Workflow, Workflows};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{
    arguments, described, tool_text, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_model::question::Answers;
use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::tool::{Tool, ToolMedia, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_model::value::identifier::Identifier;
use caelumora_runtime::component::{
    config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry,
    ComponentResult,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
pub(crate) const COMFYUI_TOOL_KIND: &str = "comfyui_tool";
/// The one tool of the component.
pub(crate) const GENERATE_IMAGE: &str = "generate_image";
/// An image generated: the `prompt` and `negative_prompt` it was described by, its `width` and
/// `height` where the arguments name them, and the `workflow` and `seed` of its result - the
/// arguments' where it has none.
const IMAGE_PRESENTATION: ToolPresentation = ToolPresentation::new("image");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComfyUiToolConfig {
    /// The ComfyUI server, reached without authentication. Its timeout is how long a call may
    /// take, its queue, its run and its fetch.
    endpoint: RemoteEndpoint,
    /// The workflows the tool offers, of those the configuration names (`Workflows`); the first is
    /// the default.
    workflows: Vec<Identifier>,
    /// Whether calls require approval. Left out, they do not: a call changes nothing but the
    /// server's time, and what leaves is its arguments, which `destination` bounds.
    #[serde(default = "approved_by_none")]
    requires_approval: ApprovalMode,
    /// What the server is cleared for: where the arguments of a call go - and stay, in its history
    /// and in the metadata of what it saves. Left out, public.
    #[serde(default)]
    destination: Option<Classification>,
    /// What the tool is for, as its tool set tells it; left out, the catalogue's text.
    #[serde(default)]
    description: Option<String>,
}

fn approved_by_none() -> ApprovalMode {
    ApprovalMode::Never
}

/// The arguments of a call of `generate_image`: the workflow, and what it is asked.
#[derive(Debug, Deserialize)]
struct Call {
    #[serde(default)]
    workflow: Option<String>,
    #[serde(flatten)]
    arguments: Arguments,
}

/// The tool `generate_image` over the workflows a component names, run on one ComfyUI server.
#[derive(Debug)]
struct ComfyUiTool {
    /// The workflow a call that names none is run with: the first named.
    default: Arc<Workflow>,
    /// Every workflow named, in their order.
    offered: Vec<Arc<Workflow>>,
    destination: Classification,
    client: ComfyUiClient,
    description: String,
    tool: Tool,
}

impl ComfyUiTool {
    /// The tool configured by `config`, over those of `workflows` it names, its texts from
    /// `prompts`.
    ///
    /// # Errors
    /// `InvalidConfig` for an endpoint with authentication, no workflow, or one `workflows` does
    /// not have; what a text that cannot be had reports.
    fn new(
        config: ComfyUiToolConfig,
        workflows: &Workflows,
        prompts: &PromptTexts,
    ) -> ComponentResult<Self> {
        if config.endpoint.auth.is_some() {
            return Err(ComponentError::InvalidConfig(
                "'endpoint.auth' is not supported: ComfyUI is reached without authentication"
                    .to_string(),
            ));
        }
        let offered = config
            .workflows
            .iter()
            .map(|name| {
                workflows.named(name.as_str()).map(Arc::clone).ok_or_else(|| {
                    ComponentError::InvalidConfig(format!(
                        "'workflows' names '{name}', which the section comfyui does not configure"
                    ))
                })
            })
            .collect::<ComponentResult<Vec<_>>>()?;
        let Some(default) = offered.first().map(Arc::clone) else {
            return Err(ComponentError::InvalidConfig("'workflows' names none".to_string()));
        };
        let values = json!({
            "workflows": offered
                .iter()
                .map(|w| json!({ "name": w.name().as_str(), "description": w.description() }))
                .collect::<Vec<_>>(),
            "default": default.name().as_str(),
        });
        let tool = generate_image(prompts, config.requires_approval.required(false), &values)?;
        let description = match config.description {
            Some(description) => description,
            None => tool_text(prompts, NAMESPACE, COMFYUI_TOOL_KIND, "description", ())?,
        };
        Ok(Self {
            default,
            offered,
            destination: config.destination.unwrap_or(Classification::PUBLIC),
            client: ComfyUiClient::new(config.endpoint),
            description,
            tool,
        })
    }

    /// The workflow named `name`, of those offered; the default if none is named.
    ///
    /// # Errors
    /// `InvalidParameter` for a name of none offered, naming those that are.
    fn workflow(&self, name: Option<&str>) -> ToolResult<&Workflow> {
        let Some(name) = name else { return Ok(&self.default) };
        match self.offered.iter().find(|workflow| workflow.name().as_str() == name) {
            Some(workflow) => Ok(workflow),
            None => {
                let names: Vec<&str> = self.offered.iter().map(|w| w.name().as_str()).collect();
                Err(ToolError::InvalidParameter(format!(
                    "there is no workflow '{name}'; there are: {}",
                    names.join(", ")
                )))
            }
        }
    }
}

/// The tool `generate_image`, its texts rendered with `values`.
fn generate_image(
    prompts: &PromptTexts,
    requires_approval: bool,
    values: &Value,
) -> Result<Tool, PromptError> {
    let tool = Tool {
        name: GENERATE_IMAGE.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [
                ("prompt", ToolParameters::string()),
                ("workflow", ToolParameters::string()),
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
    described(prompts, NAMESPACE, COMFYUI_TOOL_KIND, tool, values)
}

/// What ComfyUI failed with, as the model is told it: the server's or the workflow's failure,
/// never the model's to correct.
impl From<ComfyUiError> for ToolError {
    fn from(error: ComfyUiError) -> Self {
        ToolError::ExecutionFailed(error.to_string())
    }
}

#[async_trait]
impl ToolProvider for ComfyUiTool {
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
        let Call { workflow, arguments } = arguments(params)?;
        let workflow = self.workflow(workflow.as_deref())?;
        let graph = workflow.graph_with(&arguments)?;
        let images = self.client.generate(graph, workflow.output()).await?;
        let value = json!({
            "workflow": workflow.name().as_str(),
            "seed": arguments.seed,
            "images": images.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        });
        let media = images.into_iter().map(|(name, content)| ToolMedia { name, content }).collect();
        Ok(ToolOutput { media, ..ToolOutput::classified(value, turn.situation().label()) })
    }
}

/// Registers the component kind `comfyui_tool`, which takes the workflows of the section
/// `comfyui` as a service and writes for the agent's model.
pub(crate) fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(COMFYUI_TOOL_KIND)
                .provides::<dyn ToolProvider>()
                .uses::<Workflows>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let config: ComfyUiToolConfig = config_of_params(&descriptor.params)?;
            let workflows = context.service::<Workflows>()?;
            let tool = ComfyUiTool::new(config, workflows, &prompts)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tool));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts;
    use crate::workflow::MAX_SEED;
    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use caelumora_agents::testing::detached_actor_runtime;
    use caelumora_agents::tool::ToolOutput;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::work::Work;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use std::collections::HashMap;
    use std::path::Path as FilePath;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\ndrawn";

    /// How the ComfyUI of a test treats a prompt.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Behaviour {
        /// Done at once, with an image at the output node 9 and a preview at node 4.
        Done,
        /// Refused at node 27, before it runs.
        Refused,
        /// Failed as it ran, at node 3.
        Fails,
        /// Running, never done.
        Runs,
        /// Waiting in the queue, never run.
        Waits,
        /// Running, while its history fails to answer.
        Lost,
    }

    /// A ComfyUI of a test: what it was given, and which prompts were ended how.
    #[derive(Debug)]
    struct Fake {
        behaviour: Behaviour,
        given: Mutex<Vec<Value>>,
        ended: Mutex<Vec<String>>,
    }

    async fn queue_prompt(State(fake): State<Arc<Fake>>, Json(body): Json<Value>) -> Response {
        fake.given.lock().unwrap().push(body["prompt"].clone());
        if fake.behaviour == Behaviour::Refused {
            let refused = json!({
                "error": { "message": "Prompt outputs failed validation", "details": "" },
                "node_errors": { "27": {
                    "class_type": "CLIPTextEncode",
                    "errors": [{ "message": "Required input is missing", "details": "clip" }]
                } }
            });
            return (StatusCode::BAD_REQUEST, Json(refused)).into_response();
        }
        Json(json!({ "prompt_id": "p1", "number": 0, "node_errors": {} })).into_response()
    }

    async fn history(State(fake): State<Arc<Fake>>, Path(id): Path<String>) -> Response {
        if fake.behaviour == Behaviour::Lost {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let image =
            |file: &str, kind: &str| json!({ "filename": file, "subfolder": "c", "type": kind });
        Json(match fake.behaviour {
            Behaviour::Done => json!({ id: {
                "status": { "status_str": "success", "completed": true },
                "outputs": {
                    "9": { "images": [image("drawn.png", "output")] },
                    "4": { "images": [image("preview.png", "temp")] }
                }
            } }),
            Behaviour::Fails => json!({ id: {
                "status": { "status_str": "error", "messages": [
                    ["execution_start", {}],
                    ["execution_error", {
                        "node_id": "3",
                        "node_type": "KSampler",
                        "exception_message": "out of memory\n"
                    }]
                ] },
                "outputs": {}
            } }),
            _ => json!({}),
        })
        .into_response()
    }

    async fn view(Query(query): Query<HashMap<String, String>>) -> Response {
        match query.get("filename").map(String::as_str) {
            Some("drawn.png" | "preview.png") => PNG.into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn queue(State(fake): State<Arc<Fake>>) -> Json<Value> {
        let held = json!([[0, "p1", {}, {}, []]]);
        Json(match fake.behaviour {
            Behaviour::Runs | Behaviour::Lost => {
                json!({ "queue_running": held, "queue_pending": [] })
            }
            Behaviour::Waits => json!({ "queue_running": [], "queue_pending": held }),
            _ => json!({ "queue_running": [], "queue_pending": [] }),
        })
    }

    async fn delete(State(fake): State<Arc<Fake>>, Json(body): Json<Value>) -> StatusCode {
        let id = body["delete"][0].as_str().unwrap();
        fake.ended.lock().unwrap().push(format!("delete {id}"));
        StatusCode::OK
    }

    async fn interrupt(State(fake): State<Arc<Fake>>, Json(body): Json<Value>) -> StatusCode {
        fake.ended
            .lock()
            .unwrap()
            .push(format!("interrupt {}", body["prompt_id"].as_str().unwrap()));
        StatusCode::OK
    }

    /// A ComfyUI that treats a prompt as `behaviour` says, serving at the URL returned.
    async fn comfyui(behaviour: Behaviour) -> (Arc<Fake>, String) {
        let fake = Arc::new(Fake { behaviour, given: Mutex::default(), ended: Mutex::default() });
        let router = Router::new()
            .route("/prompt", post(queue_prompt))
            .route("/history/{id}", get(history))
            .route("/view", get(view))
            .route("/queue", get(queue).post(delete))
            .route("/interrupt", post(interrupt))
            .with_state(Arc::clone(&fake));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (fake, url)
    }

    /// The workflows of the example configuration.
    fn examples() -> Workflows {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../examples/comfyui/workflows");
        Workflows::read(&dir).unwrap()
    }

    fn built(params: Value) -> ComponentResult<ComfyUiTool> {
        ComfyUiTool::new(config_of_params(&params)?, &examples(), &prompts())
    }

    /// The tool of the example's workflow on the ComfyUI at `url`, waiting `timeout_secs`.
    fn tool(url: &str, timeout_secs: u64) -> ComfyUiTool {
        let endpoint = json!({ "url": url, "timeout_secs": timeout_secs });
        built(json!({ "endpoint": endpoint, "workflows": ["z_image_turbo"] })).unwrap()
    }

    /// What `tool` gives for a call of `generate_image` with `params`, in a turn that reaches no
    /// channel.
    async fn generated(tool: &ComfyUiTool, params: Value) -> ToolResult<ToolOutput> {
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            Default::default(),
            None,
            Default::default(),
            Default::default(),
            Default::default(),
        ));
        let mut turn = Turn::of(
            AgentId::default(),
            "test-agent".to_string(),
            &mut state,
            AgentStateHandle::detached(),
            detached_actor_runtime(),
        )
        .expect("a turn was begun");
        tool.execute(GENERATE_IMAGE, &params, None, &mut turn).await
    }

    /// Waits until `fake` ended a prompt: how.
    async fn ended(fake: &Fake) -> Vec<String> {
        time::timeout(Duration::from_secs(5), async {
            loop {
                let ended = fake.ended.lock().unwrap().clone();
                if !ended.is_empty() {
                    break ended;
                }
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the prompt is ended")
    }

    #[tokio::test]
    async fn an_image_is_generated_with_the_arguments_written_into_its_workflow() {
        let (fake, url) = comfyui(Behaviour::Done).await;
        let tool = tool(&url, 10);
        let params =
            json!({ "prompt": "a lighthouse", "seed": 7, "width": 768, "negative_prompt": "fog" });
        let output = generated(&tool, params).await.expect("generated");
        let [image] = output.media.as_slice() else { panic!("one image: {:?}", output.media) };
        assert_eq!(
            (image.name.as_str(), image.content.as_ref()),
            ("drawn.png", PNG),
            "the output node's alone"
        );
        assert_eq!(
            output.value,
            json!({ "workflow": "z_image_turbo", "seed": 7, "images": ["drawn.png"] })
        );
        let given = fake.given.lock().unwrap()[0].clone();
        assert_eq!(given["27"]["inputs"]["text"], "a lighthouse");
        assert_eq!(given["3"]["inputs"]["seed"], 7);
        assert_eq!(
            (given["13"]["inputs"]["width"].clone(), given["13"]["inputs"]["height"].clone()),
            (json!(768), json!(1024)),
            "the workflow's own height"
        );
        assert!(
            !given.to_string().contains("fog"),
            "a negative prompt the workflow does not take is left out"
        );
        assert!(fake.ended.lock().unwrap().is_empty(), "nothing to end");

        let output = generated(&tool, json!({ "prompt": "a lighthouse" })).await.unwrap();
        let seed = output.value["seed"].as_u64().expect("the seed taken");
        assert!(seed < MAX_SEED);
        assert_eq!(fake.given.lock().unwrap()[1]["3"]["inputs"]["seed"], seed, "chosen at random");
    }

    #[tokio::test]
    async fn what_comfyui_refuses_or_fails_with_is_told_the_model() {
        let (_, url) = comfyui(Behaviour::Refused).await;
        let refused = generated(&tool(&url, 10), json!({ "prompt": "x" })).await.unwrap_err();
        let at_the_node = "node 27 (CLIPTextEncode): Required input is missing clip";
        assert!(
            matches!(&refused, ToolError::ExecutionFailed(why) if why.contains(at_the_node)),
            "{refused}"
        );
        let (_, url) = comfyui(Behaviour::Fails).await;
        let failed = generated(&tool(&url, 10), json!({ "prompt": "x" })).await.unwrap_err();
        let at_the_node = "node 3 (KSampler): out of memory";
        assert!(
            matches!(&failed, ToolError::ExecutionFailed(why) if why.contains(at_the_node)),
            "{failed}"
        );
        let unreachable = tool("http://user:secret@127.0.0.1:1", 10);
        let unreachable = generated(&unreachable, json!({ "prompt": "x" })).await.unwrap_err();
        let why = unreachable.to_string();
        assert!(why.contains("'http://127.0.0.1:1'") && !why.contains("secret"), "{why}");
    }

    /// A prompt not done in time, of a call dropped, or whose history fails to answer, is ended in
    /// ComfyUI - by the id ComfyUI gave it: interrupted by it where it runs, removed from the queue
    /// where it waits.
    #[tokio::test]
    async fn a_prompt_given_up_is_ended_in_comfyui_and_no_other() {
        let (fake, url) = comfyui(Behaviour::Runs).await;
        let late = generated(&tool(&url, 1), json!({ "prompt": "x" })).await.unwrap_err();
        assert!(late.to_string().contains("did not finish within 1 s"), "{late}");
        assert_eq!(ended(&fake).await, ["interrupt p1"], "by its id");

        let (fake, url) = comfyui(Behaviour::Lost).await;
        let lost = generated(&tool(&url, 60), json!({ "prompt": "x" })).await.unwrap_err();
        assert!(lost.to_string().contains("answered 500"), "{lost}");
        assert_eq!(ended(&fake).await, ["interrupt p1"], "still running, so ended");

        let (fake, url) = comfyui(Behaviour::Waits).await;
        let tool = tool(&url, 60);
        let dropped =
            time::timeout(Duration::from_millis(700), generated(&tool, json!({ "prompt": "x" })));
        assert!(dropped.await.is_err(), "still waiting as it is dropped");
        assert_eq!(ended(&fake).await, ["delete p1"], "taken from the queue");
    }

    #[tokio::test]
    async fn what_a_call_names_is_the_models_to_correct() {
        let tool = tool("http://127.0.0.1:1", 10);
        let unknown =
            generated(&tool, json!({ "prompt": "x", "workflow": "flux" })).await.unwrap_err();
        let named = "there are: z_image_turbo";
        assert!(
            matches!(&unknown, ToolError::InvalidParameter(why) if why.contains(named)),
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
    }

    #[test]
    fn the_tool_offers_the_workflows_it_names_and_asks_nobody() {
        let tool = tool("http://127.0.0.1:1", 10);
        let [generate] = tool.tools().try_into().unwrap();
        assert!(!generate.requires_approval, "asks nobody unless configured");
        assert_eq!(generate.presentation, IMAGE_PRESENTATION);
        assert!(
            generate.description.ends_with(
                "generated with:\n- z_image_turbo (the default): fast and photorealistic, good at \
                 text in images"
            ),
            "one workflow a line: {}",
            generate.description
        );
        assert_eq!(tool.destination(), Some(Classification::PUBLIC));

        let endpoint = json!({ "url": "http://x" });
        let refused = |params: Value| built(params).unwrap_err().to_string();
        let with = |workflows: Value| json!({ "endpoint": endpoint, "workflows": workflows });
        let authenticated = json!({
            "endpoint": { "url": "http://x", "auth": { "type": "bearer", "secret": "T" } },
            "workflows": ["z_image_turbo"]
        });
        assert!(refused(authenticated).contains("endpoint.auth"));
        assert!(refused(with(json!(["flux"]))).contains("'flux'"));
        assert!(refused(with(json!([]))).contains("names none"));
        assert!(refused(with(json!(["Flux"]))).contains("invalid parameters"));
        let mut asked = with(json!(["z_image_turbo"]));
        asked["requires_approval"] = json!(true);
        assert!(built(asked).unwrap().tools()[0].requires_approval);
    }
}
