//! The `open_terminal_shared` provider (issue 251, part A): one Open
//! Terminal instance, started outside Caelumora, that has the storage at
//! `mount`; a workspace is the directory `<mount>/<id>` there. Every
//! workspace of the storage lies in the one instance, so a command in one
//! reaches every other: fit for a trusted team, one classification per
//! storage.

use super::client::{Client, Key};
use super::OpenTerminal;
use caelumora_model::remote::RemoteEndpoint;
use crate::terminal::{Terminal, TerminalError, TerminalProvider, TerminalResult};
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::secret::SecretManager;
use caelumora_model::workspace::WorkspaceId;
use reqwest::{Method, StatusCode};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

#[derive(Debug)]
pub(crate) struct SharedProvider {
    client: Arc<Client>,
    /// Absolute in the instance, without a trailing `/`.
    mount: String,
    destination: Classification,
}

impl SharedProvider {
    /// The provider of a storage at `mount` in the instance at `endpoint`,
    /// whose key is resolved through `secrets`; what may be put into it at
    /// most `destination`, `public` unless given - a command may send
    /// anything anywhere. `mount` is as the configuration checked it
    /// (`StorageConfig::check`).
    ///
    /// # Errors
    /// `Unavailable` for an endpoint that is no URL.
    pub(crate) fn new(endpoint: RemoteEndpoint, mount: &str, destination: Option<Classification>, secrets: Arc<SecretManager>) -> TerminalResult<Self> {
        let mount = mount.trim_end_matches('/').to_string();
        let client = Arc::new(Client::new(endpoint, Key::Endpoint(secrets), false)?);
        Ok(Self { client, mount, destination: destination.unwrap_or(Classification::PUBLIC) })
    }

    fn place(&self, id: WorkspaceId) -> String {
        format!("{}/{id}", self.mount)
    }

    fn terminal(&self, id: WorkspaceId) -> OpenTerminal {
        OpenTerminal { client: Arc::clone(&self.client), base: self.place(id), workspace: id }
    }
}

#[async_trait]
impl TerminalProvider for SharedProvider {
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()> {
        let place = self.place(id);
        let answer = self.client.send(Method::POST, &["files", "mkdir"], &[], None, Duration::ZERO, |r| r.json(&super::wire::PathRequest { path: &place })).await?;
        self.client.success(answer).map(|_| ())
    }

    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()> {
        let answer = self.client.send(Method::DELETE, &["files", "delete"], &[("path", self.place(id))], None, Duration::ZERO, |r| r).await?;
        match answer.status() {
            StatusCode::NOT_FOUND => Ok(()),
            _ => self.client.success(answer).map(|_| ()),
        }
    }

    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>> {
        let terminal = self.terminal(id);
        match terminal.listing(&crate::terminal::WorkspacePath::root()).await? {
            Some(_) => Ok(Arc::new(terminal)),
            None => {
                warn!(workspace = %id, "The place of a workspace is gone from its Open Terminal");
                Err(TerminalError::Unavailable("its place is gone".to_string()))
            }
        }
    }

    fn has_shell(&self) -> bool {
        true
    }

    fn destination(&self) -> Option<&Classification> {
        Some(&self.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::remote::RemoteAuthConfig;
    use crate::terminal::open_terminal::stand_in::{self, StandIn, KEY, MOUNT};
    use crate::terminal::{ByteStream, CommandStatus, WorkspacePath};
    use bytes::Bytes;
    use caelumora_model::security::DummySecurityManager;
    use futures::StreamExt;

    /// Secrets that hold the stand-in's key, and one other.
    fn secrets(key: &str) -> Arc<SecretManager> {
        #[derive(Debug)]
        struct Fixed(String);
        #[async_trait]
        impl caelumora_model::secret::SecretProvider for Fixed {
            async fn get_secret(&self, key: &str) -> caelumora_model::secret::SecretResult<Option<String>> {
                Ok((key == "OPEN_TERMINAL_API_KEY").then(|| self.0.clone()))
            }
        }
        Arc::new(SecretManager::new(vec![Arc::new(Fixed(key.to_string()))], Arc::new(DummySecurityManager::new())))
    }

    fn endpoint(url: &str) -> RemoteEndpoint {
        RemoteEndpoint { auth: Some(RemoteAuthConfig::Bearer { secret: "OPEN_TERMINAL_API_KEY".to_string() }), ..RemoteEndpoint::new(url) }
    }

    async fn workspace() -> (StandIn, SharedProvider, Arc<dyn Terminal>, WorkspaceId) {
        let stand_in = stand_in::start().await;
        let provider = SharedProvider::new(endpoint(&stand_in.url), MOUNT, None, secrets(KEY)).unwrap();
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let terminal = provider.terminal_for(id).await.unwrap();
        (stand_in, provider, terminal, id)
    }

    fn path(text: &str) -> WorkspacePath {
        WorkspacePath::parse(text).unwrap()
    }

    fn content(text: &'static str) -> ByteStream {
        Box::pin(futures::stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
    }

    async fn text(terminal: &dyn Terminal, at: &str) -> String {
        let (size, stream) = terminal.read(&path(at)).await.expect(at);
        let bytes: Vec<u8> = stream.collect::<Vec<_>>().await.into_iter().flat_map(|c| c.unwrap().to_vec()).collect();
        assert_eq!(size, bytes.len() as u64, "its size told");
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn files_are_written_read_listed_moved_and_deleted_as_locally() {
        let (stand_in, _, terminal, id) = workspace().await;
        assert!(stand_in.root.path().join(id.to_string()).is_dir(), "its place made under the mount");
        assert_eq!(terminal.write(&path("docs/a.md"), content("hello"), 100).await.unwrap(), 5, "and its directory made");
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "hello");
        terminal.write(&path("docs/a.md"), content("again"), 100).await.expect("replaced");
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "again");

        let listed = terminal.list(&WorkspacePath::root()).await.unwrap();
        assert_eq!(listed.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect::<Vec<_>>(), [("docs", true, 0)]);
        assert!(listed[0].modified.is_some());
        assert_eq!(terminal.stat(&path("docs/a.md")).await.unwrap().size, 5);
        assert!(matches!(terminal.list(&path("docs/a.md")).await, Err(TerminalError::NotADirectory(p)) if p == "docs/a.md"));
        assert!(matches!(terminal.list(&path("nowhere")).await, Err(TerminalError::NotFound(p)) if p == "nowhere"));
        assert!(matches!(terminal.read(&path("docs")).await, Err(TerminalError::IsADirectory(_))));
        assert!(matches!(terminal.write(&path("docs"), content("x"), 10).await, Err(TerminalError::IsADirectory(_))));
        assert!(matches!(terminal.write(&path("docs/a.md/x"), content("x"), 10).await, Err(TerminalError::NotADirectory(p)) if p == "docs/a.md"));

        terminal.move_entry(&path("docs/a.md"), &path("archive/2026/a.md")).await.expect("its directory made");
        assert!(matches!(terminal.stat(&path("docs/a.md")).await, Err(TerminalError::NotFound(p)) if p == "docs/a.md"));
        terminal.write(&path("docs/b.md"), content("b"), 100).await.unwrap();
        assert!(matches!(terminal.move_entry(&path("docs/b.md"), &path("archive/2026/a.md")).await, Err(TerminalError::Exists(_))), "nothing is moved over");
        assert!(matches!(terminal.move_entry(&path("gone"), &path("x")).await, Err(TerminalError::NotFound(p)) if p == "gone"));

        assert!(matches!(terminal.delete(&path("docs")).await, Err(TerminalError::NotEmpty(_))), "not removed with what is in it");
        assert!(stand_in.root.path().join(id.to_string()).join("docs/b.md").is_file());
        terminal.delete(&path("docs/b.md")).await.unwrap();
        terminal.delete(&path("docs")).await.expect("empty now");
        assert!(matches!(terminal.delete(&path("docs")).await, Err(TerminalError::NotFound(_))));
        assert!(matches!(terminal.delete(&WorkspacePath::root()).await, Err(TerminalError::InvalidPath(_))), "not the workspace itself");
        terminal.mkdir(&path("x/y")).await.unwrap();
        terminal.mkdir(&path("x/y")).await.expect("there already is no error");
        assert!(matches!(terminal.mkdir(&path("archive/2026/a.md")).await, Err(TerminalError::Exists(_))));
    }

    #[tokio::test]
    async fn a_write_beyond_the_limit_sends_nothing() {
        let (_stand_in, _, terminal, _) = workspace().await;
        terminal.write(&path("a.txt"), content("old"), 100).await.unwrap();
        assert!(matches!(terminal.write(&path("a.txt"), content("far too long"), 4).await, Err(TerminalError::TooLarge { limit: 4 })));
        assert_eq!(text(terminal.as_ref(), "a.txt").await, "old");
    }

    #[tokio::test]
    async fn a_place_is_made_and_removed_with_everything_in_it() {
        let (stand_in, provider, terminal, id) = workspace().await;
        terminal.write(&path("a/b/c.txt"), content("c"), 10).await.unwrap();
        provider.remove(id).await.unwrap();
        assert!(!stand_in.root.path().join(id.to_string()).exists());
        provider.remove(id).await.expect("nothing there is no error");
        assert!(matches!(provider.terminal_for(id).await, Err(TerminalError::Unavailable(_))));
    }

    #[tokio::test]
    async fn a_command_runs_in_the_workspace_with_the_instance_s_key_blanked() {
        let (stand_in, _, terminal, id) = workspace().await;
        let shell = terminal.shell().expect("an instance has a shell");
        let started = shell.start("channel-a", "echo made > made.txt; printf 'line\\n'; echo \"key=$OPEN_TERMINAL_API_KEY\"; exit 3").await.unwrap();
        let state = shell.output("channel-a", &started, 0, Duration::from_secs(5)).await.unwrap();
        assert_eq!((state.status, state.exit_code), (CommandStatus::Completed, Some(3)));
        assert_eq!(state.output, "line\nkey=\n", "carriage returns dropped, the key not given");
        assert!(stand_in.root.path().join(id.to_string()).join("made.txt").is_file(), "run in the workspace");
        {
            let seen = stand_in.seen.lock().unwrap();
            let (cwd, env) = seen.cwd.values().next().zip(seen.env.values().next()).unwrap();
            assert_eq!(cwd, &format!("{MOUNT}/{id}"));
            assert_eq!((env["TERM"].as_str(), env["CAELUMORA_WORKSPACE"].as_str()), ("dumb", id.to_string().as_str()));
        }

        let again = shell.output("channel-a", &started, state.next_offset, Duration::ZERO).await.unwrap();
        assert_eq!(again.output, "", "nothing new after the offset");
        assert!(matches!(shell.output("channel-b", &started, 0, Duration::ZERO).await, Err(TerminalError::UnknownCommand(c)) if c == started), "not seen from another channel");
    }

    #[tokio::test]
    async fn a_long_command_is_followed_and_killed_with_what_it_started() {
        let (_stand_in, _, terminal, _) = workspace().await;
        let shell = terminal.shell().unwrap();
        let id = shell.start("c", "echo begun; sleep 30 & sleep 30").await.unwrap();
        let state = shell.output("c", &id, 0, Duration::from_millis(300)).await.unwrap();
        assert_eq!((state.status, state.output.as_str()), (CommandStatus::Running, "begun\n"));
        let killed = shell.kill("c", &id, state.next_offset).await.unwrap();
        assert_eq!(killed.status, CommandStatus::Killed);
        assert!(matches!(shell.kill("c", "nonesuch", 0).await, Err(TerminalError::UnknownCommand(_))));
    }

    #[tokio::test]
    async fn a_refused_key_and_an_instance_out_of_reach_are_told_without_the_endpoint() {
        let stand_in = stand_in::start().await;
        let refused = SharedProvider::new(endpoint(&stand_in.url), MOUNT, None, secrets("wrong")).unwrap();
        let error = refused.create(WorkspaceId::new()).await.unwrap_err();
        assert!(matches!(&error, TerminalError::Unavailable(why) if why.contains("refused the storage's key")), "{error}");
        let away = SharedProvider::new(endpoint("http://user:secret@127.0.0.1:9"), MOUNT, None, secrets(KEY)).unwrap();
        let error = away.create(WorkspaceId::new()).await.unwrap_err().to_string();
        assert!(error.contains("not reached") && !error.contains("secret") && !error.contains("127.0.0.1"), "{error}");
        assert!(matches!(SharedProvider::new(endpoint("no url"), MOUNT, None, secrets(KEY)), Err(TerminalError::Unavailable(_))));
        assert_eq!(refused.destination(), Some(&Classification::PUBLIC), "public unless given");
    }

    #[tokio::test]
    async fn an_error_of_the_instance_names_nothing_inside_it() {
        let (_stand_in, _, terminal, _) = workspace().await;
        terminal.write(&path("f"), content("x"), 10).await.unwrap();
        let error = terminal.mkdir(&path("f/g")).await.unwrap_err().to_string();
        assert!(!error.contains(MOUNT) && !error.contains("/tmp"), "{error}");
    }

    /// The chain against a real instance, by hand: `CAELUMORA_OPEN_TERMINAL_URL`
    /// names it, its key is `OPEN_TERMINAL_API_KEY` in `local/secrets.json`,
    /// and the storage lies at `/tmp/caelumora-workspaces` in it - gone with
    /// the container.
    #[tokio::test]
    #[ignore = "needs a running Open Terminal"]
    async fn against_a_real_instance() {
        let url = std::env::var("CAELUMORA_OPEN_TERMINAL_URL").expect("CAELUMORA_OPEN_TERMINAL_URL");
        let file = caelumora_model::secret::file::FileSecretProvider::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../local/secrets.json"));
        let secrets = Arc::new(SecretManager::new(vec![Arc::new(file)], Arc::new(DummySecurityManager::new())));
        let provider = SharedProvider::new(endpoint(&url), "/tmp/caelumora-workspaces", None, secrets).unwrap();
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let terminal = provider.terminal_for(id).await.unwrap();

        terminal.write(&path("docs/a.md"), content("hello"), 100).await.unwrap();
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "hello");
        assert_eq!(terminal.list(&path("docs")).await.unwrap().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a.md"]);
        assert!(matches!(terminal.list(&path("docs/a.md")).await, Err(TerminalError::NotADirectory(_))));
        assert!(matches!(terminal.read(&path("docs")).await, Err(TerminalError::IsADirectory(_))));
        assert!(matches!(terminal.delete(&path("docs")).await, Err(TerminalError::NotEmpty(_))));
        terminal.move_entry(&path("docs/a.md"), &path("b/a.md")).await.unwrap();

        let shell = terminal.shell().unwrap();
        let scope = uuid::Uuid::new_v4().to_string();
        let started = shell.start(&scope, "cat b/a.md; echo; echo \"key=[$OPEN_TERMINAL_API_KEY]\"; pwd; exit 3").await.unwrap();
        let state = shell.output(&scope, &started, 0, Duration::from_secs(10)).await.unwrap();
        assert_eq!((state.status, state.exit_code), (CommandStatus::Completed, Some(3)), "{state:?}");
        assert_eq!(state.output, format!("hello\nkey=[]\n/tmp/caelumora-workspaces/{id}\n"));
        assert!(matches!(shell.output("elsewhere", &started, 0, Duration::ZERO).await, Err(TerminalError::UnknownCommand(_))));

        let long = shell.start(&scope, "echo begun; sleep 60 & sleep 60").await.unwrap();
        let running = shell.output(&scope, &long, 0, Duration::from_millis(500)).await.unwrap();
        assert_eq!((running.status, running.output.as_str()), (CommandStatus::Running, "begun\n"));
        let killed = shell.kill(&scope, &long, running.next_offset).await.unwrap();
        assert_eq!(killed.status, CommandStatus::Killed);
        let left = shell.start(&scope, "pgrep -c -f '[s]leep 60' || true").await.unwrap();
        let left = shell.output(&scope, &left, 0, Duration::from_secs(10)).await.unwrap();
        assert_eq!(left.output.trim(), "0", "what it started went with it");

        provider.remove(id).await.unwrap();
        assert!(matches!(provider.terminal_for(id).await, Err(TerminalError::Unavailable(_))));
    }
}
