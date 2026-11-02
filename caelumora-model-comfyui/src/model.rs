//! The backend `comfyui` (issue 525): an image model the platform offers, one workflow
//! (`workflow`) run on one ComfyUI server (`client`).
//!
//! Parameters: `endpoint`, the ComfyUI server, reached without authentication, whose `timeout_secs`
//! bounds a whole call - its queue, its run and its fetch, 300 seconds unless given -, and
//! `workflow`, the path of the workflow's two files without their extension, read as the seed files
//! are, from where the program runs. The model describes itself as its workflow does.

use crate::client::{ComfyUiClient, ComfyUiError};
use crate::workflow::Workflow;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::image::{Image, ImageRequest};
use caelumora_api::model::{ModelError, ModelResult};
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_kernel::backend::Backends;
use caelumora_model::ImageModelProvider;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// The type of the backend in the configuration.
pub(crate) const COMFYUI_BACKEND: &str = "comfyui";

/// The parameters of a model of the backend `comfyui`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComfyUiParams {
    endpoint: RemoteEndpoint,
    workflow: PathBuf,
}

impl Params for ComfyUiParams {
    /// # Errors
    /// `InvalidConfig` for an endpoint with authentication.
    fn check(&self) -> ComponentResult<()> {
        if self.endpoint.auth.is_some() {
            return Err(ComponentError::InvalidConfig(
                "'endpoint.auth' is not supported: ComfyUI is reached without authentication"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// An image model served by ComfyUI: one workflow on one server.
#[derive(Debug)]
struct ComfyUiModel {
    /// Its name, as the configuration gives it.
    name: String,
    workflow: Workflow,
    /// The server, as it may be shown.
    endpoint: String,
    client: ComfyUiClient,
}

impl ComfyUiModel {
    /// What `error`, of talking to the server, is as a model's failure.
    fn failed(&self, error: ComfyUiError) -> ModelError {
        match error {
            ComfyUiError::Unreachable { endpoint, reason } => {
                ModelError::Connection { endpoint, reason }
            }
            ComfyUiError::Rejected { endpoint, status, reason } => {
                ModelError::Rejected { endpoint, model: self.name.clone(), status, message: reason }
            }
            ComfyUiError::Unreadable { endpoint, answer } => {
                ModelError::InvalidResponse { endpoint, reason: format!("it answered {answer}") }
            }
            other => ModelError::Backend {
                endpoint: self.endpoint.clone(),
                model: self.name.clone(),
                message: other.to_string(),
            },
        }
    }
}

#[async_trait]
impl ImageModelProvider for ComfyUiModel {
    fn description(&self) -> &str {
        self.workflow.description()
    }

    /// Writes `request` into the workflow's graph, runs it and gives the images of its output
    /// node; a call dropped ends its prompt in ComfyUI.
    async fn generate(&self, request: &ImageRequest) -> ModelResult<Vec<Image>> {
        let graph = self
            .workflow
            .graph_with(request)
            .map_err(|reason| ModelError::Unfit { model: self.name.clone(), reason })?;
        self.client.generate(graph, self.workflow.output()).await.map_err(|e| self.failed(e))
    }
}

/// Registers the backend `comfyui` of the image models with `backends`: each model its workflow,
/// read and checked as the kernel is built.
pub(crate) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn ImageModelProvider, ComfyUiParams, _>(
        COMFYUI_BACKEND,
        |name, params, _| {
            let workflow = Workflow::read(&params.workflow)
                .map_err(|e| ComponentError::InvalidConfig(format!("'workflow': {e}")))?;
            Ok(Arc::new(ComfyUiModel {
                name: name.to_string(),
                workflow,
                endpoint: params.endpoint.shown(),
                client: ComfyUiClient::new(params.endpoint),
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use caelumora_api::component::Services;
    use caelumora_domain::config::BackendConfig;
    use serde_json::{json, Value};
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

    /// The model `z_image_turbo` of the backend `comfyui` configured with `params`, the workflow
    /// of the example configuration unless they name another.
    fn model_with(mut params: Value) -> ComponentResult<Arc<dyn ImageModelProvider>> {
        if params.get("workflow").is_none() {
            let example = FilePath::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/comfyui/workflows/z_image_turbo");
            params["workflow"] = json!(example);
        }
        let mut backends = Backends::new();
        register_backend(&mut backends);
        let config = BackendConfig::new(COMFYUI_BACKEND, params);
        backends.make_one("image model", "z_image_turbo", &config, &Services::empty())
    }

    /// The model of the example's workflow on the ComfyUI at `url`, waiting `timeout_secs`.
    fn model(url: &str, timeout_secs: u64) -> Arc<dyn ImageModelProvider> {
        model_with(json!({ "endpoint": { "url": url, "timeout_secs": timeout_secs } })).unwrap()
    }

    /// A request for "a lighthouse", of `width` where given.
    fn request(width: Option<u64>) -> ImageRequest {
        ImageRequest {
            prompt: "a lighthouse".to_string(),
            negative_prompt: Some("fog".to_string()),
            width,
            height: None,
            seed: 7,
        }
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
    async fn an_image_is_generated_with_the_request_written_into_its_workflow() {
        let (fake, url) = comfyui(Behaviour::Done).await;
        let images = model(&url, 10).generate(&request(Some(768))).await.expect("generated");
        let [image] = images.as_slice() else { panic!("one image: {images:?}") };
        assert_eq!(
            (image.name.as_str(), image.content.as_ref()),
            ("drawn.png", PNG),
            "the output node's alone"
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
    }

    #[tokio::test]
    async fn what_comfyui_refuses_or_fails_with_is_the_model_s_failure() {
        let (_, url) = comfyui(Behaviour::Refused).await;
        let refused = model(&url, 10).generate(&request(None)).await.unwrap_err();
        let at_the_node = "node 27 (CLIPTextEncode): Required input is missing clip";
        assert!(
            matches!(&refused, ModelError::Backend { model, message, .. }
                if model == "z_image_turbo" && message.contains(at_the_node)),
            "{refused}"
        );
        let (_, url) = comfyui(Behaviour::Fails).await;
        let failed = model(&url, 10).generate(&request(None)).await.unwrap_err();
        let at_the_node = "node 3 (KSampler): out of memory";
        assert!(
            matches!(&failed, ModelError::Backend { message, .. } if message.contains(at_the_node)),
            "{failed}"
        );
        let unreachable = model("http://user:secret@127.0.0.1:1", 10);
        let unreachable = unreachable.generate(&request(None)).await.unwrap_err();
        assert!(matches!(unreachable, ModelError::Connection { .. }), "{unreachable}");
        let why = unreachable.to_string();
        assert!(why.contains("http://127.0.0.1:1") && !why.contains("secret"), "{why}");
    }

    /// A prompt not done in time, of a call dropped, or whose history fails to answer, is ended in
    /// ComfyUI - by the id ComfyUI gave it: interrupted by it where it runs, removed from the queue
    /// where it waits.
    #[tokio::test]
    async fn a_prompt_given_up_is_ended_in_comfyui_and_no_other() {
        let (fake, url) = comfyui(Behaviour::Runs).await;
        let late = model(&url, 1).generate(&request(None)).await.unwrap_err();
        assert!(late.to_string().contains("did not finish within 1 s"), "{late}");
        assert_eq!(ended(&fake).await, ["interrupt p1"], "by its id");

        let (fake, url) = comfyui(Behaviour::Lost).await;
        let lost = model(&url, 60).generate(&request(None)).await.unwrap_err();
        assert!(
            matches!(&lost, ModelError::Rejected { model, status: 500, .. }
                if model == "z_image_turbo"),
            "{lost}"
        );
        assert_eq!(ended(&fake).await, ["interrupt p1"], "still running, so ended");

        let (fake, url) = comfyui(Behaviour::Waits).await;
        let model = model(&url, 60);
        let request = request(None);
        let dropped = time::timeout(Duration::from_millis(700), model.generate(&request));
        assert!(dropped.await.is_err(), "still waiting as it is dropped");
        assert_eq!(ended(&fake).await, ["delete p1"], "taken from the queue");
    }

    /// What the model does not take is told before anything is sent, for the asker to correct.
    #[tokio::test]
    async fn a_size_outside_the_workflow_s_bounds_is_not_taken() {
        let wide = model("http://127.0.0.1:1", 10).generate(&request(Some(4096))).await;
        let wide = wide.unwrap_err();
        assert!(
            matches!(&wide, ModelError::Unfit { model, reason }
                if model == "z_image_turbo" && reason.contains("512 to 2048")),
            "{wide}"
        );
    }

    /// The model describes itself as its workflow does; a server with authentication, or a
    /// workflow that cannot be read, keeps it from being made, naming the parameter.
    #[test]
    fn a_model_is_its_workflow_on_its_server() {
        let model = model("http://127.0.0.1:1", 10);
        assert!(model.description().starts_with("fast and photorealistic"));
        let refused = |params: Value| model_with(params).unwrap_err().to_string();
        let authenticated = json!({
            "endpoint": { "url": "http://x", "auth": { "type": "bearer", "secret": "T" } }
        });
        assert!(refused(authenticated).contains("endpoint.auth"));
        let unreadable =
            refused(json!({ "endpoint": { "url": "http://x" }, "workflow": "/none/flux" }));
        assert!(
            unreadable.contains("'workflow'") && unreadable.contains("'flux.json'"),
            "{unreadable}"
        );
        assert!(!unreadable.contains("/none"), "no path: {unreadable}");
    }
}
