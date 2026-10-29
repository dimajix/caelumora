//! The `open_terminal_docker` provider (issue 251, part A): a volume and an
//! Open Terminal container per workspace, started by Caelumora over the
//! Docker daemon its environment names (`DOCKER_HOST`, else the local
//! socket). A workspace's commands are kept to its container; what the
//! container's network reaches, they reach.
//!
//! - `create` makes the volume `caelumora-ws-<id>`; `remove` takes the
//!   container and the volume away.
//! - `terminal_for` starts the container on first use - the image pulled if
//!   it is missing, the volume at `/workspace`, made writable for the
//!   image's user, labelled with the storage and the workspace, with a key
//!   made for it - and waits until it answers. The key lives in the
//!   container's configuration, where only who holds the daemon reads it,
//!   so a container outlives Caelumora and is found again after a restart.
//! - Every container is reached directly, without a proxy, at its address
//!   in `network`.
//! - A task stops a container unused for `idle_stop`, and any running
//!   container of the storage that no workspace in use here holds - one
//!   left from an earlier run. Stopping a container ends the commands in it,
//!   which are forgotten then; ending one when told (issue 254) does not
//!   start a container that is not running.

use crate::client::{Client, Key};
use crate::terminal::{OpenCommands, OpenTerminal};
use async_trait::async_trait;
use bollard::errors::Error as DockerError;
use bollard::exec::{CreateExecOptions, StartExecOptions};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, HostConfig, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, ListContainersOptions, RemoveContainerOptions,
    RemoveVolumeOptions, StopContainerOptions,
};
use bollard::Docker;
use caelumora_model::confidentiality::Classification;
use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::workspace::WorkspaceId;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::component::{config_of_params, ComponentError, ComponentResult};
use caelumora_runtime::terminal::{
    default_max_run_secs, max_run, CommandKey, Terminal, TerminalError, TerminalProvider,
    TerminalResult,
};
use futures::StreamExt;
use reqwest::Method;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tokio::time::{sleep, Instant};
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, warn};
use uuid::Uuid;

/// The label naming the storage a container or volume belongs to.
const STORAGE_LABEL: &str = "caelumora.storage";
/// The label naming the workspace a container or volume belongs to.
const WORKSPACE_LABEL: &str = "caelumora.workspace";
/// Where a container has its workspace.
const BASE: &str = "/workspace";
/// The port Open Terminal listens on in its image.
const PORT: u16 = 8000;
/// How long a container may take to answer once started - its entrypoint
/// may install packages first.
const START_WAIT: Duration = Duration::from_secs(120);
/// How long a container is given to stop before it is killed.
const STOP_GRACE_SECS: i32 = 5;

/// The parameters of `open_terminal_docker`: the image the containers run,
/// the network they are started in, how long one may stay unused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DockerStorageConfig {
    /// Pinned: Open Terminal's API is 0.x.
    image: String,
    /// The Docker network the containers are started in, which reaches
    /// what the agents are meant to reach; Caelumora reaches them at their
    /// address there.
    #[serde(default = "default_network")]
    network: String,
    #[serde(default = "default_idle_stop_secs")]
    idle_stop_secs: u64,
    /// What may be put into this storage; `public` unless given - a
    /// command reaches whatever its network does.
    #[serde(default)]
    destination: Option<Classification>,
    /// How long a command may run (issue 254), bounded by its container
    /// itself.
    #[serde(default = "default_max_run_secs")]
    max_run_secs: u64,
}

fn default_network() -> String {
    "bridge".to_string()
}

fn default_idle_stop_secs() -> u64 {
    900
}

impl DockerStorageConfig {
    /// Nothing if its fields fit: an image is named, the network gives a
    /// container an address of its own, and a container may stay unused a
    /// while.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` naming what does not fit.
    fn check(&self) -> ComponentResult<()> {
        let invalid = |why: String| Err(ComponentError::InvalidConfig(why));
        if self.image.trim().is_empty() {
            return invalid("an image is needed".to_string());
        }
        let network = &self.network;
        if matches!(network.as_str(), "" | "host" | "none") || network.starts_with("container:") {
            return invalid(format!(
                "the network '{network}' does not give a container an address of its own"
            ));
        }
        if self.idle_stop_secs == 0 {
            return invalid("idle_stop_secs must be more than 0".to_string());
        }
        Ok(())
    }
}

/// Registers the storage `open_terminal_docker` with `backends`: its
/// containers are labelled with the name of the storage.
pub(crate) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn TerminalProvider, _>("open_terminal_docker", |storage, params, _| {
        let config: DockerStorageConfig = config_of_params(params)?;
        config.check()?;
        let max_run = max_run(config.max_run_secs)?;
        let provider = DockerProvider::new(
            storage,
            &config.image,
            &config.network,
            Duration::from_secs(config.idle_stop_secs),
            max_run,
            config.destination,
        )
        .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
        Ok(Arc::new(provider))
    });
}

/// The instance of a workspace while it runs here: its client, or none.
type Slot = Arc<Mutex<Option<Arc<Client>>>>;

#[derive(Debug)]
pub(crate) struct DockerProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    docker: Docker,
    storage: String,
    image: String,
    network: String,
    idle_stop: Duration,
    /// The commands its workspaces were given, which run for `max_run` at
    /// most.
    commands: Arc<OpenCommands>,
    max_run: Duration,
    destination: Classification,
    /// The workspaces used here, each with its instance while it runs.
    slots: StdMutex<HashMap<WorkspaceId, Slot>>,
    /// The task that stops what is not used; ends with the provider.
    reaper: OnceLock<AbortOnDropHandle<()>>,
}

/// The name of the container and of the volume of workspace `id`.
fn name(id: WorkspaceId) -> String {
    format!("caelumora-ws-{id}")
}

/// Whether `error` is the daemon saying that what was asked for is not there.
fn missing(error: &DockerError) -> bool {
    matches!(error, DockerError::DockerResponseServerError { status_code: 404, .. })
}

/// That the daemon failed to `what`: logged in full, told without what the
/// daemon wrote - it names paths of the host.
fn failed(what: &str, error: DockerError) -> TerminalError {
    warn!("Docker could not {what}: {error}");
    TerminalError::Unavailable(format!("Docker could not {what}"))
}

impl DockerProvider {
    /// The provider of the storage `storage`, whose containers run `image`
    /// in `network` and stop after `idle_stop` unused, and whose commands
    /// run for `max_run` at most; what may be put into it at most
    /// `destination`, `public` unless given. Starts the task that stops
    /// what is not used - also what an earlier run left running.
    ///
    /// # Errors
    /// `Unavailable` outside a Tokio runtime, or where no Docker daemon can
    /// be addressed.
    pub(crate) fn new(
        storage: &str,
        image: &str,
        network: &str,
        idle_stop: Duration,
        max_run: Duration,
        destination: Option<Classification>,
    ) -> TerminalResult<Self> {
        let runtime = Handle::try_current().map_err(|_| {
            TerminalError::Unavailable("a Docker storage needs a running runtime".to_string())
        })?;
        let docker =
            Docker::connect_with_local_defaults().map_err(|e| failed("be addressed", e))?;
        let inner = Arc::new(Inner {
            docker,
            storage: storage.to_string(),
            image: image.to_string(),
            network: network.to_string(),
            idle_stop,
            commands: Arc::default(),
            max_run,
            destination: destination.unwrap_or(Classification::PUBLIC),
            slots: StdMutex::default(),
            reaper: OnceLock::new(),
        });
        let every = (idle_stop / 4).clamp(Duration::from_secs(1), Duration::from_secs(60));
        let _ = inner
            .reaper
            .set(AbortOnDropHandle::new(runtime.spawn(reap(Arc::downgrade(&inner), every))));
        Ok(Self { inner })
    }
}

/// Stops, every `every`, what `inner` does not use, until it is gone.
async fn reap(inner: Weak<Inner>, every: Duration) {
    loop {
        let Some(inner) = inner.upgrade() else { return };
        inner.reap().await;
        drop(inner);
        sleep(every).await;
    }
}

impl Inner {
    fn labels(&self, id: WorkspaceId) -> HashMap<String, String> {
        HashMap::from([
            (STORAGE_LABEL.to_string(), self.storage.clone()),
            (WORKSPACE_LABEL.to_string(), id.to_string()),
        ])
    }

    /// The terminal of workspace `id` in the instance behind `client`.
    fn terminal(&self, id: WorkspaceId, client: Arc<Client>) -> OpenTerminal {
        OpenTerminal {
            client,
            base: BASE.to_string(),
            workspace: id,
            commands: Arc::clone(&self.commands),
            max_run: self.max_run,
        }
    }

    fn slot(&self, id: WorkspaceId) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(slots.entry(id).or_default())
    }

    /// The client of workspace `id`'s instance, started if it does not run.
    async fn instance(&self, id: WorkspaceId) -> TerminalResult<Arc<Client>> {
        let slot = self.slot(id);
        let mut held = slot.lock().await;
        if let Some(client) = held.as_ref() {
            return Ok(Arc::clone(client));
        }
        let client = Arc::new(self.start(id).await?);
        *held = Some(Arc::clone(&client));
        Ok(client)
    }

    /// Starts the container of workspace `id`, made if there is none, and
    /// answers its client once it answers.
    async fn start(&self, id: WorkspaceId) -> TerminalResult<Client> {
        let name = name(id);
        let container = match self.docker.inspect_container(&name, None).await {
            Ok(container) => container,
            Err(e) if missing(&e) => {
                self.make(id).await?;
                self.inspect(&name).await?
            }
            Err(e) => return Err(failed("inspect a workspace's container", e)),
        };
        let labels = container.config.as_ref().and_then(|c| c.labels.as_ref());
        if labels.and_then(|l| l.get(STORAGE_LABEL)) != Some(&self.storage) {
            warn!(workspace = %id, container = %name, "A container of a workspace's name is not of its storage");
            return Err(TerminalError::Unavailable(
                "a container of its name belongs to something else".to_string(),
            ));
        }
        let key = container
            .config
            .as_ref()
            .and_then(|c| c.env.as_ref())
            .and_then(|env| env.iter().find_map(|var| var.strip_prefix("OPEN_TERMINAL_API_KEY=")))
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                TerminalError::Unavailable("a workspace's container has no key".to_string())
            })?
            .to_string();
        let container = match container.state.as_ref().and_then(|s| s.running) {
            Some(true) => container,
            _ => {
                info!(workspace = %id, storage = %self.storage, "Starting the container of a workspace");
                self.docker
                    .start_container(&name, None)
                    .await
                    .map_err(|e| failed("start a workspace's container", e))?;
                self.inspect(&name).await?
            }
        };
        self.give_workspace(&name).await?;
        let address = container
            .network_settings
            .as_ref()
            .and_then(|settings| settings.networks.as_ref())
            .and_then(|networks| networks.get(&self.network))
            .and_then(|endpoint| endpoint.ip_address.clone())
            .filter(|address| !address.is_empty())
            .ok_or_else(|| {
                TerminalError::Unavailable(format!(
                    "a workspace's container has no address in the network '{}'",
                    self.network
                ))
            })?;
        let client = Client::new(
            RemoteEndpoint::new(format!("http://{address}:{PORT}")),
            Key::Made(key),
            true,
        )?;
        self.ready(&client).await?;
        Ok(client)
    }

    async fn inspect(&self, name: &str) -> TerminalResult<ContainerInspectResponse> {
        self.docker
            .inspect_container(name, None)
            .await
            .map_err(|e| failed("inspect a workspace's container", e))
    }

    /// Makes the container of workspace `id`, with a key of its own; the
    /// image pulled first if the daemon does not have it.
    async fn make(&self, id: WorkspaceId) -> TerminalResult<()> {
        let name = name(id);
        let key = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let body = ContainerCreateBody {
            image: Some(self.image.clone()),
            env: Some(vec![format!("OPEN_TERMINAL_API_KEY={key}")]),
            labels: Some(self.labels(id)),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{name}:{BASE}")]),
                network_mode: Some(self.network.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options =
            || Some(CreateContainerOptions { name: Some(name.clone()), ..Default::default() });
        match self.docker.create_container(options(), body.clone()).await {
            Ok(_) => Ok(()),
            Err(e) if missing(&e) => {
                info!(image = %self.image, "Pulling the image of a Docker storage");
                let pull = CreateImageOptions {
                    from_image: Some(self.image.clone()),
                    ..Default::default()
                };
                let mut progress = self.docker.create_image(Some(pull), None, None);
                while let Some(step) = progress.next().await {
                    step.map_err(|e| failed("pull the image of the storage", e))?;
                }
                self.docker
                    .create_container(options(), body)
                    .await
                    .map(|_| ())
                    .map_err(|e| failed("make a workspace's container", e))
            }
            Err(e) => Err(failed("make a workspace's container", e)),
        }
    }

    /// Makes the workspace writable for the image's user: a volume is
    /// made owned by root.
    async fn give_workspace(&self, name: &str) -> TerminalResult<()> {
        let exec = CreateExecOptions {
            cmd: Some(vec!["chown", "user:user", BASE]),
            user: Some("root"),
            ..Default::default()
        };
        let exec = self
            .docker
            .create_exec(name, exec)
            .await
            .map_err(|e| failed("prepare a workspace's container", e))?;
        self.docker
            .start_exec(&exec.id, Some(StartExecOptions { detach: true, ..Default::default() }))
            .await
            .map_err(|e| failed("prepare a workspace's container", e))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = self
                .docker
                .inspect_exec(&exec.id)
                .await
                .map_err(|e| failed("prepare a workspace's container", e))?;
            match (state.running, state.exit_code) {
                (Some(false), Some(0)) => return Ok(()),
                (Some(false), code) => {
                    warn!(container = %name, ?code, "Giving the workspace to the image's user failed");
                    return Err(TerminalError::Unavailable(
                        "a workspace's container could not be prepared".to_string(),
                    ));
                }
                _ if Instant::now() >= deadline => {
                    return Err(TerminalError::Unavailable(
                        "a workspace's container was not prepared in time".to_string(),
                    ))
                }
                _ => sleep(Duration::from_millis(100)).await,
            }
        }
    }

    /// Waits until the instance behind `client` answers.
    async fn ready(&self, client: &Client) -> TerminalResult<()> {
        let deadline = Instant::now() + START_WAIT;
        loop {
            if let Ok(answer) =
                client.send(Method::GET, &["health"], &[], None, Duration::ZERO, |r| r).await
            {
                if answer.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(TerminalError::Unavailable(
                    "a workspace's container did not answer in time".to_string(),
                ));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    async fn stop(&self, name: &str) {
        info!(container = %name, storage = %self.storage, "Stopping the unused container of a workspace");
        let options = StopContainerOptions { t: Some(STOP_GRACE_SECS), ..Default::default() };
        match self.docker.stop_container(name, Some(options)).await {
            Ok(()) => {}
            Err(e) if missing(&e) => {}
            Err(e) => warn!(container = %name, "An unused container could not be stopped: {e}"),
        }
    }

    /// Stops the containers unused for `idle_stop`, and the running ones of
    /// the storage no workspace in use here holds.
    async fn reap(&self) {
        let slots: Vec<(WorkspaceId, Slot)> = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, slot)| (*id, Arc::clone(slot)))
            .collect();
        for (id, slot) in &slots {
            // One being started or used right now is not idle.
            let Ok(mut held) = slot.try_lock() else { continue };
            if held.as_ref().is_some_and(|client| client.idle_for() >= self.idle_stop) {
                self.stop(&name(*id)).await;
                self.commands.forget(*id);
                *held = None;
            }
        }
        let filters = HashMap::from([(
            "label".to_string(),
            vec![format!("{STORAGE_LABEL}={}", self.storage)],
        )]);
        let running = match self
            .docker
            .list_containers(Some(ListContainersOptions {
                filters: Some(filters),
                ..Default::default()
            }))
            .await
        {
            Ok(running) => running,
            Err(e) => {
                return warn!(storage = %self.storage, "The containers of a Docker storage could not be listed: {e}")
            }
        };
        for container in running {
            let Some(id) = container
                .labels
                .as_ref()
                .and_then(|l| l.get(WORKSPACE_LABEL))
                .and_then(|id| id.parse::<Uuid>().ok())
                .map(WorkspaceId::from)
            else {
                continue;
            };
            let held = slots.iter().find(|(used, _)| *used == id).map(|(_, slot)| slot);
            let unheld = match held {
                None => true,
                Some(slot) => slot.try_lock().is_ok_and(|held| held.is_none()),
            };
            if unheld {
                self.stop(&name(id)).await;
            }
        }
    }
}

#[async_trait]
impl TerminalProvider for DockerProvider {
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()> {
        let volume = VolumeCreateRequest {
            name: Some(name(id)),
            labels: Some(self.inner.labels(id)),
            ..Default::default()
        };
        self.inner
            .docker
            .create_volume(volume)
            .await
            .map(|_| ())
            .map_err(|e| failed("make a workspace's volume", e))
    }

    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()> {
        let slot = self.inner.slot(id);
        let mut held = slot.lock().await;
        *held = None;
        let name = name(id);
        match self
            .inner
            .docker
            .remove_container(
                &name,
                Some(RemoveContainerOptions { force: true, ..Default::default() }),
            )
            .await
        {
            Ok(()) => {}
            Err(e) if missing(&e) => {}
            Err(e) => return Err(failed("remove a workspace's container", e)),
        }
        match self.inner.docker.remove_volume(&name, None::<RemoveVolumeOptions>).await {
            Ok(()) => {}
            Err(e) if missing(&e) => {}
            Err(e) => return Err(failed("remove a workspace's volume", e)),
        }
        self.inner.commands.forget(id);
        drop(held);
        self.inner.slots.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id);
        Ok(())
    }

    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>> {
        let client = self.inner.instance(id).await?;
        Ok(Arc::new(self.inner.terminal(id, client)))
    }

    fn has_shell(&self) -> bool {
        true
    }

    fn commands(&self) -> Vec<CommandKey> {
        self.inner.commands.running()
    }

    async fn end_command(&self, (workspace, scope, id): &CommandKey) -> TerminalResult<()> {
        let slot = self
            .inner
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(workspace)
            .map(Arc::clone);
        let client = match slot {
            Some(slot) => slot.lock().await.clone(),
            None => None,
        };
        // Not running: what ran in it ended with it.
        let Some(client) = client else {
            self.inner.commands.forget(*workspace);
            return Ok(());
        };
        match self.inner.terminal(*workspace, client).shell(scope) {
            Some(shell) => shell.end(id).await,
            None => Ok(()),
        }
    }

    fn destination(&self) -> Option<&Classification> {
        Some(&self.inner.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::OpenTerminalPlugin;
    use bytes::Bytes;
    use caelumora_model::config::BackendConfig;
    use caelumora_model::workspace::CommandStatus;
    use caelumora_runtime::runtime::Services;
    use caelumora_runtime::terminal::{ByteStream, WorkspacePath, DEFAULT_MAX_RUN_SECS};
    use futures::stream;
    use serde_json::json;
    use std::env;
    use std::slice;
    use tokio::time::timeout;

    fn path(text: &str) -> WorkspacePath {
        WorkspacePath::parse(text).unwrap()
    }

    fn content(text: &'static str) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
    }

    async fn running(docker: &Docker, id: WorkspaceId) -> bool {
        docker
            .inspect_container(&name(id), None)
            .await
            .ok()
            .and_then(|c| c.state)
            .and_then(|s| s.running)
            == Some(true)
    }

    /// The whole life of a workspace's container, against the Docker daemon
    /// of the environment: `CAELUMORA_DOCKER_TESTS` set, the image from
    /// `CAELUMORA_DOCKER_IMAGE` (the published 0.14.0 unless given). What it
    /// makes, it removes.
    #[tokio::test]
    #[ignore = "needs a Docker daemon"]
    async fn a_container_per_workspace_is_started_stopped_found_again_and_removed() {
        if env::var_os("CAELUMORA_DOCKER_TESTS").is_none() {
            return eprintln!("CAELUMORA_DOCKER_TESTS is not set");
        }
        let image = env::var("CAELUMORA_DOCKER_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/open-webui/open-terminal:0.14.0".to_string());
        let storage = format!("test-{}", Uuid::new_v4().simple());
        let provider = DockerProvider::new(
            &storage,
            &image,
            "bridge",
            Duration::from_secs(3),
            Duration::from_secs(3600),
            None,
        )
        .unwrap();
        let docker = provider.inner.docker.clone();
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();

        let terminal = provider.terminal_for(id).await.expect("started");
        assert!(running(&docker, id).await);
        terminal
            .write(&path("docs/a.md"), content("hello"), 100)
            .await
            .expect("the workspace is the image user's to write");
        let shell = terminal.shell("c").unwrap();
        let started = shell
            .start("cat docs/a.md; echo; pwd; echo \"[$OPEN_TERMINAL_API_KEY]\"; touch made")
            .await
            .unwrap();
        let state = shell.output(&started, 0, Duration::from_secs(10)).await.unwrap();
        assert_eq!(
            (state.status, state.output.as_str()),
            (CommandStatus::Completed, "hello\n/workspace\n[]\n")
        );
        let long = shell.start("sleep 600").await.unwrap();
        let key = (id, "c".to_string(), long);
        assert_eq!(provider.commands(), slice::from_ref(&key), "what may still run");

        // Unused, it is stopped, with the commands in it; used again,
        // started with the same key and the same files - also by a provider
        // of a later run.
        timeout(Duration::from_secs(30), async {
            while running(&docker, id).await {
                sleep(Duration::from_millis(500)).await
            }
        })
        .await
        .expect("stopped when unused");
        assert!(provider.commands().is_empty(), "ended with its container");
        provider.end_command(&key).await.expect("one that ended is no error");
        assert!(!running(&docker, id).await, "and its container is not started for it");
        let later = DockerProvider::new(
            &storage,
            &image,
            "bridge",
            Duration::from_secs(600),
            Duration::from_secs(3600),
            None,
        )
        .unwrap();
        drop(provider);
        let terminal = later.terminal_for(id).await.expect("started again");
        let listed: Vec<String> = terminal
            .list(&WorkspacePath::root())
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(listed, ["docs", "made"]);

        later.remove(id).await.unwrap();
        assert!(
            matches!(docker.inspect_container(&name(id), None).await, Err(e) if missing(&e)),
            "the container is gone"
        );
        assert!(
            matches!(docker.inspect_volume(&name(id)).await, Err(e) if missing(&e)),
            "and the volume"
        );
        later.remove(id).await.expect("nothing there is no error");
    }

    /// A running container of the storage that no workspace in use holds -
    /// one an earlier run left - is stopped.
    #[tokio::test]
    #[ignore = "needs a Docker daemon"]
    async fn what_an_earlier_run_left_running_is_stopped() {
        if env::var_os("CAELUMORA_DOCKER_TESTS").is_none() {
            return eprintln!("CAELUMORA_DOCKER_TESTS is not set");
        }
        let image = env::var("CAELUMORA_DOCKER_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/open-webui/open-terminal:0.14.0".to_string());
        let storage = format!("test-{}", Uuid::new_v4().simple());
        let earlier = DockerProvider::new(
            &storage,
            &image,
            "bridge",
            Duration::from_secs(600),
            Duration::from_secs(3600),
            None,
        )
        .unwrap();
        let docker = earlier.inner.docker.clone();
        let id = WorkspaceId::new();
        earlier.create(id).await.unwrap();
        earlier.terminal_for(id).await.unwrap();
        drop(earlier);
        assert!(running(&docker, id).await, "left running");

        let later = DockerProvider::new(
            &storage,
            &image,
            "bridge",
            Duration::from_secs(600),
            Duration::from_secs(3600),
            None,
        )
        .unwrap();
        timeout(Duration::from_secs(30), async {
            while running(&docker, id).await {
                sleep(Duration::from_millis(500)).await
            }
        })
        .await
        .expect("stopped at start");
        later.remove(id).await.unwrap();
    }

    /// A command that runs as long as it may is killed by its container
    /// itself, with what it started (issue 254).
    #[tokio::test]
    #[ignore = "needs a Docker daemon"]
    async fn a_command_is_killed_by_its_container_once_it_ran_as_long_as_it_may() {
        if env::var_os("CAELUMORA_DOCKER_TESTS").is_none() {
            return eprintln!("CAELUMORA_DOCKER_TESTS is not set");
        }
        let image = env::var("CAELUMORA_DOCKER_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/open-webui/open-terminal:0.14.0".to_string());
        let storage = format!("test-{}", Uuid::new_v4().simple());
        let provider = DockerProvider::new(
            &storage,
            &image,
            "bridge",
            Duration::from_secs(600),
            Duration::from_secs(2),
            None,
        )
        .unwrap();
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let shell = provider.terminal_for(id).await.unwrap().shell("c").unwrap();

        let long = shell.start("echo \"begun 'quoted'\"; sleep 41 & sleep 41").await.unwrap();
        let ended = shell.output(&long, 0, Duration::from_secs(20)).await.unwrap();
        assert_eq!(
            (ended.status, ended.output.as_str()),
            (CommandStatus::Killed, "begun 'quoted'\n"),
            "{ended:?}"
        );
        assert!(provider.commands().is_empty(), "no longer listed");
        let left = shell.start("pgrep -c -f '[s]leep 41' || true").await.unwrap();
        assert_eq!(
            shell.output(&left, 0, Duration::from_secs(10)).await.unwrap().output.trim(),
            "0",
            "what it started went with it"
        );
        provider.remove(id).await.unwrap();
    }

    #[test]
    fn its_parameters_name_its_image_and_take_the_rest_by_default() {
        let config: DockerStorageConfig =
            config_of_params(&json!({ "image": "ghcr.io/open-webui/open-terminal:0.14.0" }))
                .expect("a container per workspace");
        config.check().expect("fits");
        assert_eq!(
            (
                config.image.as_str(),
                config.network.as_str(),
                config.idle_stop_secs,
                &config.destination,
                config.max_run_secs
            ),
            ("ghcr.io/open-webui/open-terminal:0.14.0", "bridge", 900, &None, DEFAULT_MAX_RUN_SECS)
        );
        for (wrong, why) in [
            (json!({ "network": "host" }), "network"),
            (json!({ "network": "container:x" }), "network"),
            (json!({ "idle_stop_secs": 0 }), "idle_stop_secs"),
            (json!({ "image": " " }), "image"),
        ] {
            let mut params = json!({ "image": "ghcr.io/open-webui/open-terminal:0.14.0" });
            params.as_object_mut().unwrap().extend(wrong.as_object().unwrap().clone());
            let config: DockerStorageConfig = config_of_params(&params).unwrap();
            let refused = config.check().unwrap_err().to_string();
            assert!(refused.contains(why), "{refused}");
        }
        let storages = [(
            "isolated".to_string(),
            BackendConfig::new("open_terminal_docker", json!({ "image": "x", "max_run_secs": 0 })),
        )]
        .into();
        let refused = Backends::with_plugins(&[&OpenTerminalPlugin])
            .make_named::<dyn TerminalProvider>(
                "storage of workspaces",
                &storages,
                &Services::empty(),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(refused.contains("'isolated'") && refused.contains("max_run_secs"), "{refused}");
    }
}
