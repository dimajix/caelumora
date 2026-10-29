//! A stand-in for Open Terminal 0.14.0, for the tests: the routes the
//! provider uses, answered as the instance answers them - read from its
//! sources - over a temporary directory that it has at `/workspaces`.
//! Commands run for real, under `sh -c`, their output given as a
//! pseudo-terminal gives it (`\r\n`), one piece per read.

use axum::extract::{Multipart, Path as RoutePath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::time::{sleep, Instant};
use uuid::Uuid;

pub(crate) const MOUNT: &str = "/workspaces";
pub(crate) const KEY: &str = "stand-in-key";

#[derive(Debug, Default)]
struct Process {
    session: String,
    status: String,
    exit_code: Option<i32>,
    entries: Vec<String>,
    group: Option<i32>,
}

#[derive(Default)]
pub(crate) struct Seen {
    /// The environment each command was given, by its command.
    pub(crate) env: HashMap<String, HashMap<String, String>>,
    /// The working directory each command was given, by its command.
    pub(crate) cwd: HashMap<String, String>,
}

#[derive(Clone)]
struct Instance {
    root: PathBuf,
    processes: Arc<Mutex<HashMap<String, Arc<Mutex<Process>>>>>,
    seen: Arc<Mutex<Seen>>,
}

pub(crate) struct StandIn {
    pub(crate) url: String,
    /// Where the instance's `/workspaces` lies on the host.
    pub(crate) root: TempDir,
    pub(crate) seen: Arc<Mutex<Seen>>,
}

impl Instance {
    /// Where `path`, absolute in the instance, lies on the host; `None`
    /// outside `/workspaces`.
    fn host(&self, path: &str) -> Option<PathBuf> {
        let rest = path.strip_prefix(MOUNT)?;
        Some(self.root.join(rest.trim_start_matches('/')))
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {KEY}"))
}

fn session(headers: &HeaderMap) -> String {
    headers.get("x-session-id").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
}

fn detail(status: StatusCode, text: &str) -> Response {
    (status, Json(json!({ "detail": text }))).into_response()
}

macro_rules! guard {
    ($headers:expr) => {
        if !authorized(&$headers) {
            return detail(StatusCode::UNAUTHORIZED, "Invalid API key");
        }
    };
}

fn modified(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

async fn list(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    guard!(headers);
    let directory = query.get("directory").cloned().unwrap_or_default();
    let Some(dir) = instance.host(&directory).filter(|d| d.is_dir()) else {
        return detail(StatusCode::NOT_FOUND, "Directory not found");
    };
    let mut entries = Vec::new();
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let full = dir.join(&name);
        let is_dir = full.is_dir();
        entries.push(json!({ "name": name, "type": if is_dir { "directory" } else { "file" }, "size": fs::metadata(&full).map(|m| m.len()).unwrap_or(0), "modified": modified(&full), "writable": true }));
    }
    Json(json!({ "dir": directory, "writable": true, "entries": entries })).into_response()
}

async fn view(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    guard!(headers);
    let Some(file) = query.get("path").and_then(|p| instance.host(p)).filter(|f| f.is_file())
    else {
        return detail(StatusCode::NOT_FOUND, "File not found");
    };
    ([("content-type", "application/octet-stream")], fs::read(file).unwrap()).into_response()
}

async fn upload(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Response {
    guard!(headers);
    let Some(dir) = query.get("directory").and_then(|p| instance.host(p)) else {
        return detail(StatusCode::FORBIDDEN, "Access denied");
    };
    let Ok(Some(field)) = multipart.next_field().await else {
        return detail(StatusCode::UNPROCESSABLE_ENTITY, "file missing");
    };
    let name = Path::new(field.file_name().unwrap_or("upload")).file_name().unwrap().to_owned();
    let content = field.bytes().await.unwrap();
    if let Err(e) = fs::create_dir_all(&dir) {
        return detail(StatusCode::BAD_REQUEST, &format!("{e}: {}", dir.display()));
    }
    match fs::write(dir.join(name), &content) {
        Ok(()) => Json(json!({ "path": "...", "size": content.len() })).into_response(),
        Err(e) => detail(StatusCode::BAD_REQUEST, &format!("{e}: {}", dir.display())),
    }
}

async fn remove(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    guard!(headers);
    let Some(target) = query.get("path").and_then(|p| instance.host(p)).filter(|t| t.exists())
    else {
        return detail(StatusCode::NOT_FOUND, "Path not found");
    };
    let is_dir = target.is_dir();
    if is_dir {
        fs::remove_dir_all(&target).unwrap()
    } else {
        fs::remove_file(&target).unwrap()
    }
    Json(json!({ "path": "...", "type": if is_dir { "directory" } else { "file" } }))
        .into_response()
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

async fn mkdir(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Response {
    guard!(headers);
    let Some(dir) = instance.host(&body.path) else {
        return detail(StatusCode::FORBIDDEN, "Access denied");
    };
    match fs::create_dir_all(&dir) {
        Ok(()) => Json(json!({ "path": body.path })).into_response(),
        Err(e) => detail(StatusCode::BAD_REQUEST, &format!("{e}: {}", dir.display())),
    }
}

#[derive(Deserialize)]
struct MoveBody {
    source: String,
    destination: String,
}

async fn move_entry(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Json(body): Json<MoveBody>,
) -> Response {
    guard!(headers);
    let (Some(source), Some(destination)) =
        (instance.host(&body.source), instance.host(&body.destination))
    else {
        return detail(StatusCode::FORBIDDEN, "Access denied");
    };
    if !source.exists() {
        return detail(StatusCode::NOT_FOUND, "Source path not found");
    }
    if !destination.parent().is_some_and(Path::is_dir) {
        return detail(StatusCode::BAD_REQUEST, "Destination parent directory not found");
    }
    if destination.exists() {
        return detail(StatusCode::CONFLICT, "Destination already exists");
    }
    fs::rename(source, destination).unwrap();
    Json(json!({ "source": body.source, "destination": body.destination })).into_response()
}

#[derive(Deserialize)]
struct ExecBody {
    command: String,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
}

fn answer(id: &str, process: &Process, offset: usize, tail: Option<usize>) -> Value {
    let new: Vec<&String> = process.entries.iter().skip(offset).collect();
    let kept = tail.map_or(new.len(), |t| t.min(new.len()));
    let output: Vec<Value> = new[new.len() - kept..]
        .iter()
        .map(|data| json!({ "type": "output", "data": data }))
        .collect();
    json!({
        "id": id, "command": "...", "status": process.status, "exit_code": process.exit_code,
        "output": output, "truncated": kept < new.len(), "next_offset": process.entries.len(), "log_path": "/var/log/open-terminal/x.jsonl",
    })
}

async fn wait_for(process: &Arc<Mutex<Process>>, wait: f64) {
    let until = Instant::now() + Duration::from_secs_f64(wait);
    while process.lock().unwrap().status == "running" && Instant::now() < until {
        sleep(Duration::from_millis(10)).await;
    }
}

async fn execute(
    State(instance): State<Instance>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<ExecBody>,
) -> Response {
    guard!(headers);
    let cwd = body.cwd.clone().unwrap_or_default();
    {
        let mut seen = instance.seen.lock().unwrap();
        seen.env.insert(body.command.clone(), body.env.clone().unwrap_or_default());
        seen.cwd.insert(body.command.clone(), cwd.clone());
    }
    let Some(dir) = instance.host(&cwd) else {
        return detail(StatusCode::INTERNAL_SERVER_ERROR, "no such directory");
    };
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&body.command)
        .current_dir(dir)
        .env("OPEN_TERMINAL_API_KEY", KEY)
        .envs(body.env.unwrap_or_default())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().unwrap();
    let id = format!("20261008-120000-{}", &Uuid::new_v4().simple().to_string()[..6]);
    let process = Arc::new(Mutex::new(Process {
        session: session(&headers),
        status: "running".to_string(),
        group: child.id().map(|p| p as i32),
        ..Default::default()
    }));
    instance.processes.lock().unwrap().insert(id.clone(), Arc::clone(&process));
    let mut stdout = child.stdout.take().unwrap();
    let running = Arc::clone(&process);
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdout.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            let piece = String::from_utf8_lossy(&buffer[..read]).replace('\n', "\r\n");
            running.lock().unwrap().entries.push(piece);
        }
        let status = child.wait().await.unwrap();
        let mut running = running.lock().unwrap();
        running.exit_code = Some(status.code().unwrap_or_else(|| -status.signal().unwrap_or(0)));
        if running.status != "killed" {
            running.status = "done".to_string();
        }
    });
    if let Some(wait) = query.get("wait").and_then(|w| w.parse::<f64>().ok()) {
        wait_for(&process, wait).await;
    }
    let process = process.lock().unwrap();
    Json(answer(&id, &process, 0, None)).into_response()
}

fn found(instance: &Instance, id: &str, headers: &HeaderMap) -> Option<Arc<Mutex<Process>>> {
    let process = instance.processes.lock().unwrap().get(id).cloned()?;
    let same = process.lock().unwrap().session == session(headers);
    same.then_some(process)
}

async fn status(
    State(instance): State<Instance>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    guard!(headers);
    let Some(process) = found(&instance, &id, &headers) else {
        return detail(StatusCode::NOT_FOUND, "Process not found");
    };
    if let Some(wait) = query.get("wait").and_then(|w| w.parse::<f64>().ok()) {
        wait_for(&process, wait).await;
    }
    let offset = query.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
    let tail = query.get("tail").and_then(|t| t.parse().ok());
    let process = process.lock().unwrap();
    Json(answer(&id, &process, offset, tail)).into_response()
}

async fn kill(
    State(instance): State<Instance>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> Response {
    guard!(headers);
    let Some(process) = found(&instance, &id, &headers) else {
        return detail(StatusCode::NOT_FOUND, "Process not found");
    };
    let group = {
        let mut process = process.lock().unwrap();
        if process.status != "running" {
            return Json(json!({ "status": "killed" })).into_response();
        }
        process.status = "killed".to_string();
        process.group
    };
    if let Some(group) = group {
        let _ = killpg(Pid::from_raw(group), Signal::SIGKILL);
    }
    while process.lock().unwrap().exit_code.is_none() {
        sleep(Duration::from_millis(10)).await;
    }
    Json(json!({ "status": "killed" })).into_response()
}

/// A stand-in listening on a free port of localhost.
pub(crate) async fn start() -> StandIn {
    let root = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let instance = Instance {
        root: root.path().to_path_buf(),
        processes: Arc::default(),
        seen: Arc::clone(&seen),
    };
    let app = Router::new()
        .route("/files/list", get(list))
        .route("/files/view", get(view))
        .route("/files/upload", post(upload))
        .route("/files/delete", delete(remove))
        .route("/files/mkdir", post(mkdir))
        .route("/files/move", post(move_entry))
        .route("/execute", post(execute))
        .route("/execute/{id}/status", get(status))
        .route("/execute/{id}", delete(kill))
        .with_state(instance);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    StandIn { url, root, seen }
}
