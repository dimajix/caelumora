//! A ComfyUI server reached over HTTP (issue 512): a graph queued, followed until it is done, its
//! images fetched - or the prompt ended in ComfyUI once whoever waits for it gives up.
//!
//! ComfyUI answers `POST /prompt` with the prompt's id, holds it in its queue (`GET /queue`) while
//! it waits or runs, and in its history (`GET /history/<id>`) once it ended, with the images of
//! each node; `GET /view` gives one. A prompt is ended by `POST /queue` with `delete` while it
//! waits and by `POST /interrupt` with its id while it runs - never by an interrupt without one,
//! which ends whatever runs, someone else's prompt too.

use bytes::Bytes;
use caelumora_model::remote::RemoteEndpoint;
use caelumora_runtime::remote::describe_request_error;
use regex::Regex;
use reqwest::{Client, Error, RequestBuilder, Response, Url};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::mem;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time;
use tracing::{debug, warn};
use uuid::Uuid;

/// How long a call may take unless the endpoint says otherwise: its queue, its run and its fetch.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long one request may take, within the call's own bound.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the history is asked whether a prompt ended.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// How long ending the prompt of a call given up may take.
const CANCEL_WITHIN: Duration = Duration::from_secs(5);

/// What talking to ComfyUI failed with: written to be read, naming the server without what may
/// carry a credential.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ComfyUiError {
    #[error("ComfyUI at '{endpoint}' cannot be reached: {reason}")]
    Unreachable { endpoint: String, reason: String },
    #[error("ComfyUI at '{endpoint}' answered {answer}")]
    Answered { endpoint: String, answer: String },
    /// The workflow, as ComfyUI checked it before running it.
    #[error("ComfyUI refused the workflow: {0}")]
    Refused(String),
    /// What failed as it ran.
    #[error("the workflow failed in ComfyUI: {0}")]
    Failed(String),
    #[error("the workflow gave no image")]
    NoImage,
    #[error("ComfyUI did not finish within {0} s")]
    TimedOut(u64),
}

type ComfyUiResult<T> = Result<T, ComfyUiError>;

/// An image ComfyUI keeps, as its history names it.
#[derive(Debug, Clone, Deserialize)]
struct ImageRef {
    filename: String,
    #[serde(default)]
    subfolder: String,
    #[serde(rename = "type", default = "kept_as_output")]
    kind: String,
}

/// Where an image lies that its history does not say of: among ComfyUI's outputs.
fn kept_as_output() -> String {
    "output".to_string()
}

/// Where a prompt stands, as the history tells it.
enum PromptState {
    /// Not yet in the history: waiting or running.
    Unfinished,
    Failed(String),
    /// Done, with the images of each node.
    Done(BTreeMap<String, Vec<ImageRef>>),
}

/// One ComfyUI server, reached without authentication: it knows none.
#[derive(Debug, Clone)]
pub(crate) struct ComfyUiClient {
    http: Client,
    endpoint: RemoteEndpoint,
}

impl ComfyUiClient {
    pub(crate) fn new(endpoint: RemoteEndpoint) -> Self {
        Self { http: Client::new(), endpoint }
    }

    /// The images `graph` gives - those of the node `output`, else of every node -, each by its
    /// file name with its bytes: the whole of it - queue, run and fetch - bounded by the endpoint's
    /// timeout. A prompt queued is ended in ComfyUI if the call is dropped, the time runs out or
    /// asking for it fails before it ended (`CancelOnDrop`).
    ///
    /// # Errors
    /// `Unreachable`, `Answered`; `Refused`, `Failed` as ComfyUI says; `NoImage`; `TimedOut`.
    pub(crate) async fn generate(
        &self,
        graph: Map<String, Value>,
        output: Option<&str>,
    ) -> ComfyUiResult<Vec<(String, Bytes)>> {
        let within = self.endpoint.timeout_or(DEFAULT_TIMEOUT);
        time::timeout(within, self.run(graph, output))
            .await
            .map_err(|_| ComfyUiError::TimedOut(within.as_secs()))?
    }

    /// `generate`, without the bound of time. The prompt is given an id of its own before it is
    /// queued, so that a call dropped as it is queued ends it too; ComfyUI may give it another,
    /// which the guard takes then.
    async fn run(
        &self,
        graph: Map<String, Value>,
        output: Option<&str>,
    ) -> ComfyUiResult<Vec<(String, Bytes)>> {
        let mut guard = CancelOnDrop {
            client: self.clone(),
            prompt_id: Uuid::new_v4().to_string(),
            armed: true,
        };
        match self.queue(graph, &guard.prompt_id).await {
            Ok(prompt_id) => guard.prompt_id = prompt_id,
            Err(refused @ ComfyUiError::Refused(_)) => {
                guard.disarm();
                return Err(refused);
            }
            Err(e) => return Err(e),
        }
        let ended = self.finished(&guard.prompt_id).await;
        // Done or failed, nothing is left to end; where asking failed, the guard ends it.
        if matches!(ended, Ok(_) | Err(ComfyUiError::Failed(_))) {
            guard.disarm();
        }
        let images = ended?;
        let mut chosen: Vec<ImageRef> = match output {
            Some(node) => images.get(node).cloned().unwrap_or_default(),
            None => images.into_values().flatten().collect(),
        };
        chosen.retain(|image| image.kind == "output" || image.kind == "temp");
        if chosen.is_empty() {
            return Err(ComfyUiError::NoImage);
        }
        let mut fetched = Vec::with_capacity(chosen.len());
        for image in chosen {
            let content = self.image(&image).await?;
            fetched.push((image.filename, content));
        }
        Ok(fetched)
    }

    /// Queues `graph` as the prompt `prompt_id`: the id ComfyUI gives it.
    async fn queue(&self, graph: Map<String, Value>, prompt_id: &str) -> ComfyUiResult<String> {
        let body = json!({ "prompt": graph, "prompt_id": prompt_id });
        let response = self.send(self.post("prompt").json(&body)).await?;
        if response.status().as_u16() == 400 {
            let refused: Value = response.json().await.unwrap_or_default();
            return Err(ComfyUiError::Refused(refusal(&refused)));
        }
        let queued: Value = self.read(response).await?;
        let prompt_id = queued.get("prompt_id").and_then(Value::as_str);
        let prompt_id = prompt_id.ok_or_else(|| self.answered("no prompt id"))?.to_string();
        debug!(prompt = %prompt_id, "Queued in ComfyUI");
        Ok(prompt_id)
    }

    /// The images of each node of the prompt `prompt_id`, once it ended.
    async fn finished(&self, prompt_id: &str) -> ComfyUiResult<BTreeMap<String, Vec<ImageRef>>> {
        loop {
            match self.state(prompt_id).await? {
                PromptState::Unfinished => time::sleep(POLL_EVERY).await,
                PromptState::Failed(why) => return Err(ComfyUiError::Failed(why)),
                PromptState::Done(images) => return Ok(images),
            }
        }
    }

    /// Where the prompt `prompt_id` stands.
    async fn state(&self, prompt_id: &str) -> ComfyUiResult<PromptState> {
        let response = self.send(self.get(&format!("history/{prompt_id}"))).await?;
        let history: Value = self.read(response).await?;
        let Some(entry) = history.get(prompt_id) else { return Ok(PromptState::Unfinished) };
        let status = entry.get("status");
        if status.and_then(|s| s.get("status_str")).and_then(Value::as_str) == Some("error") {
            return Ok(PromptState::Failed(failure(status)));
        }
        let outputs = entry.get("outputs").and_then(Value::as_object);
        let images = outputs
            .into_iter()
            .flatten()
            .map(|(node, output)| {
                let images = output.get("images").cloned().unwrap_or(Value::Null);
                (node.clone(), serde_json::from_value(images).unwrap_or_default())
            })
            .collect();
        Ok(PromptState::Done(images))
    }

    /// The bytes of `image`.
    async fn image(&self, image: &ImageRef) -> ComfyUiResult<Bytes> {
        let query =
            [("filename", &image.filename), ("subfolder", &image.subfolder), ("type", &image.kind)];
        let url = Url::parse_with_params(&self.url("view"), query)
            .map_err(|_| self.answered("an image no URL can name"))?;
        let response = self.send(self.http.get(url).timeout(REQUEST_TIMEOUT)).await?;
        let response = self.successful(response)?;
        response.bytes().await.map_err(|e| self.unreachable(e))
    }

    /// Ends the prompt `prompt_id`: removed from the queue where it waits, interrupted by its id
    /// where it runs, nothing where it ended already.
    async fn cancel(&self, prompt_id: &str) -> ComfyUiResult<()> {
        let queue: Value = self.read(self.send(self.get("queue")).await?).await?;
        let holds = |list: &str| {
            queue
                .get(list)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|entry| entry.get(1).and_then(Value::as_str) == Some(prompt_id))
        };
        let ending = if holds("queue_running") {
            self.post("interrupt").json(&json!({ "prompt_id": prompt_id }))
        } else if holds("queue_pending") {
            self.post("queue").json(&json!({ "delete": [prompt_id] }))
        } else {
            return Ok(());
        };
        self.successful(self.send(ending).await?)?;
        debug!(prompt = %prompt_id, "Ended in ComfyUI");
        Ok(())
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.endpoint.url.trim_end_matches('/'))
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(self.url(path)).timeout(REQUEST_TIMEOUT)
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.http.post(self.url(path)).timeout(REQUEST_TIMEOUT)
    }

    async fn send(&self, request: RequestBuilder) -> ComfyUiResult<Response> {
        request.send().await.map_err(|e| self.unreachable(e))
    }

    /// `response`, if it is a success.
    fn successful(&self, response: Response) -> ComfyUiResult<Response> {
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(self.answered(&status.to_string()))
        }
    }

    /// The JSON of `response`, a success.
    async fn read(&self, response: Response) -> ComfyUiResult<Value> {
        let response = self.successful(response)?;
        response.json().await.map_err(|_| self.answered("what is no JSON"))
    }

    fn unreachable(&self, error: Error) -> ComfyUiError {
        ComfyUiError::Unreachable {
            endpoint: self.endpoint.shown(),
            reason: self.endpoint.redact_in(&describe_request_error(error)),
        }
    }

    fn answered(&self, answer: &str) -> ComfyUiError {
        ComfyUiError::Answered { endpoint: self.endpoint.shown(), answer: answer.to_string() }
    }
}

/// What ComfyUI said of a workflow it refused: its error, and each node's.
fn refusal(refused: &Value) -> String {
    let error = refused.get("error");
    let mut said: Vec<String> =
        [error.and_then(|e| e.get("message")), error.and_then(|e| e.get("details"))]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect();
    for (node, errors) in
        refused.get("node_errors").and_then(Value::as_object).into_iter().flatten()
    {
        let class = errors.get("class_type").and_then(Value::as_str).unwrap_or("node");
        for error in errors.get("errors").and_then(Value::as_array).into_iter().flatten() {
            let message = error.get("message").and_then(Value::as_str).unwrap_or_default();
            let details = error.get("details").and_then(Value::as_str).unwrap_or_default();
            said.push(format!("node {node} ({class}): {message} {details}").trim_end().to_string());
        }
    }
    if said.is_empty() {
        "it gave no reason".to_string()
    } else {
        without_paths(&said.join("; "))
    }
}

/// What ComfyUI said of a prompt that failed as it ran: the node and its exception, or that it
/// was interrupted.
fn failure(status: Option<&Value>) -> String {
    let messages = status.and_then(|s| s.get("messages")).and_then(Value::as_array);
    for message in messages.into_iter().flatten() {
        let (Some(kind), Some(data)) = (message.get(0).and_then(Value::as_str), message.get(1))
        else {
            continue;
        };
        let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default();
        match kind {
            "execution_error" => {
                let exception = without_paths(text("exception_message").trim());
                return format!("node {} ({}): {exception}", text("node_id"), text("node_type"));
            }
            "execution_interrupted" => return "it was interrupted".to_string(),
            _ => {}
        }
    }
    "it gave no reason".to_string()
}

/// An absolute path, of a Unix host or a Windows one, of at least two parts: what ComfyUI writes
/// into its errors of the files on its host.
#[expect(clippy::expect_used, reason = "a constant pattern, which the tests compile")]
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    let part = r#"[^\s'"`<>|:,;()\[\]{}/\\]"#;
    let last = r#"(?:[^\s'"`<>|:,;()\[\]{}]*[^\s'"`<>|:,;()\[\]{}.!?])?"#;
    Regex::new(&format!(r"(?:[A-Za-z]:\\|/)(?:{part}+[/\\])+{last}")).expect("a pattern")
});

/// `text`, what ComfyUI said, with each absolute path in it written `<path>`: messages are shown
/// to the model and to users, and name no file of the server's host.
fn without_paths(text: &str) -> String {
    PATH.replace_all(text, "<path>").into_owned()
}

/// Ends a prompt in ComfyUI if dropped before `disarm` (issue 512): a call given up - its turn
/// cancelled or stopped, its time run out, its server no longer answering - would hold the server
/// for nothing. The ending runs in a task of its own, bounded and logged, which the drop does not
/// await.
struct CancelOnDrop {
    client: ComfyUiClient,
    prompt_id: String,
    armed: bool,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = Handle::try_current() else { return };
        let (client, prompt_id) = (self.client.clone(), mem::take(&mut self.prompt_id));
        runtime.spawn(async move {
            match time::timeout(CANCEL_WITHIN, client.cancel(&prompt_id)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(prompt = %prompt_id, "The prompt of a call given up was not ended: {e}"),
                Err(_) => warn!(prompt = %prompt_id, "The prompt of a call given up was not ended in time"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_comfyui_says_names_no_file_of_its_host() {
        let missing =
            "Value not in list: ckpt_name: '/opt/ComfyUI/models/checkpoints/x.safetensors' \
                       not in [], see C:\\ComfyUI\\models\\vae\\ae.safetensors.";
        assert_eq!(
            without_paths(missing),
            "Value not in list: ckpt_name: '<path>' not in [], see <path>."
        );
        assert_eq!(without_paths("out of memory / try again"), "out of memory / try again");
        let refused = json!({
            "error": { "message": "Prompt outputs failed validation", "details": "" },
            "node_errors": { "28": {
                "class_type": "UNETLoader",
                "errors": [{ "message": "No such file", "details": "/srv/models/unet/z.safetensors" }]
            } }
        });
        assert_eq!(
            refusal(&refused),
            "Prompt outputs failed validation; node 28 (UNETLoader): No such file <path>"
        );
        let failed = json!({ "messages": [["execution_error", {
            "node_id": "8",
            "node_type": "VAEDecode",
            "exception_message": "[Errno 2] No such file: '/tmp/x/y.png'\n"
        }]] });
        assert_eq!(failure(Some(&failed)), "node 8 (VAEDecode): [Errno 2] No such file: '<path>'");
    }
}
