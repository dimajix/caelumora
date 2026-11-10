//! Where documents and states lie as files (issue 554): a backend over files says so with a
//! `DocumentLayout` and a `StateLayout`, and `JsonDocuments` and `JsonStates` keep them there.

use super::to_backend_error;
use async_trait::async_trait;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::store::{EntityKind, StoreResult};
use caelumora_store::documents::Document;
use std::fmt::Debug;
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

/// Where the documents of one kind lie as JSON files, and what keeps their writers apart. A layout
/// makes no directories: what is written makes those it needs, and a directory not there holds
/// nothing.
///
/// # Errors
/// A method answering a `StoreResult` fails if the place cannot be told, and what the storage was
/// asked then fails with that error.
#[async_trait]
pub trait DocumentLayout: Debug + Send + Sync {
    /// Where the document `id` lies, if there is one.
    async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>>;

    /// Where the new `document` is to lie.
    async fn place(&self, document: &Document) -> StoreResult<PathBuf>;

    /// Every document of the kind, each with its id and where it lies.
    async fn all(&self) -> StoreResult<Vec<(Uuid, PathBuf)>>;

    /// Held by a write from reading what it judges until it has written.
    async fn writing(&self) -> StoreResult<Box<dyn Send>>;

    /// Tidies up once the file at `path` was removed; nothing unless a layout says otherwise.
    async fn removed(&self, _path: &Path) -> StoreResult<()> {
        Ok(())
    }
}

/// The layout of the documents of one kind in the JSON store: one file per document in the kind's
/// directory, `entities/<kind>`, named by its id.
#[derive(Debug)]
pub(crate) struct EntitiesLayout {
    dir: PathBuf,
    /// Held by every write: within this process, which is the only one that writes the files.
    writes: Arc<Mutex<()>>,
}

impl EntitiesLayout {
    /// The layout of the documents of `kind` under `root`.
    ///
    /// # Errors
    /// If its directory cannot be made, so that a path that cannot be written fails the start.
    pub(crate) fn new(root: &Path, kind: EntityKind) -> StoreResult<Self> {
        let dir = root.join("entities").join(kind.name());
        std_fs::create_dir_all(&dir).map_err(to_backend_error)?;
        Ok(Self { dir, writes: Default::default() })
    }

    /// Where the document `id` is.
    fn path(&self, id: Uuid) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

#[async_trait]
impl DocumentLayout for EntitiesLayout {
    async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>> {
        let path = self.path(id);
        Ok(fs::try_exists(&path).await.map_err(to_backend_error)?.then_some(path))
    }

    async fn place(&self, document: &Document) -> StoreResult<PathBuf> {
        Ok(self.path(document.id))
    }

    async fn all(&self) -> StoreResult<Vec<(Uuid, PathBuf)>> {
        documents_in_dir(&self.dir).await
    }

    async fn writing(&self) -> StoreResult<Box<dyn Send>> {
        Ok(Box::new(Arc::clone(&self.writes).lock_owned().await))
    }
}

/// The documents in `dir` - every `*.json` file named by a UUID -, each with
/// its id, the file's name without `.json`, and its path; temporary files of
/// writes in progress, or left behind by a crash, are not among them, and a
/// file of another name is left out and logged. Only the directory is read:
/// a document is read when it is its turn.
async fn documents_in_dir(dir: &Path) -> StoreResult<Vec<(Uuid, PathBuf)>> {
    let mut documents = Vec::new();
    let mut entries = fs::read_dir(dir).await.map_err(to_backend_error)?;
    loop {
        let path = match entries.next_entry().await {
            Ok(Some(e)) => e.path(),
            Ok(None) => break,
            Err(err) => {
                warn!("Failed to read directory entry in {:?}: {}", dir, err);
                continue;
            }
        };

        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        // Named as `EntitiesLayout::path` names it - hyphenated, in small letters -, so that what
        // a scan lists is found by its id.
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        match Uuid::parse_str(stem) {
            Ok(id) if id.to_string() == stem => documents.push((id, path)),
            _ => warn!("A document in {:?} is not named by its id: {:?}", dir, path.file_name()),
        }
    }
    Ok(documents)
}

/// Where the states of channels, agents and users lie. It only says where: what is written makes
/// the directories it needs, and a directory not there holds nothing. The directories `agents_in`
/// and `users_in` answer are `JsonStates`'s alone, which removes them whole with their channel, so
/// that nothing else may lie in them.
///
/// # Errors
/// A method answering a `StoreResult` fails if the place cannot be told. What the state stores
/// were asked then fails with that error, also a reading that otherwise answers a default.
#[async_trait]
pub trait StateLayout: Debug + Send + Sync {
    /// The snapshot of the state of `channel`; its journal and its history lie beside it.
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// The directory of the states of the agents in `channel`, those of each beside each other.
    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// The directory of the states of the users in `channel`.
    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// What `agents_in` answers, of every channel.
    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>>;

    /// The directory of the index of the work under way.
    fn under_way(&self) -> PathBuf;
}

/// The layout of the JSON store: every kind of state in a directory of its own, below `runtime`,
/// named by the channel it is of.
#[derive(Debug)]
pub(crate) struct RuntimeLayout {
    runtime: PathBuf,
}

impl RuntimeLayout {
    /// The layout of the states under `root`.
    ///
    /// # Errors
    /// If its directories cannot be made, so that a path that cannot be written fails the start.
    pub(crate) fn new(root: impl AsRef<Path>) -> StoreResult<Self> {
        let layout = Self { runtime: root.as_ref().join("runtime") };
        for dir in [layout.agents(), layout.channels(), layout.users(), layout.under_way()] {
            std_fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(layout)
    }

    fn agents(&self) -> PathBuf {
        self.runtime.join("agent_channel_state")
    }

    fn channels(&self) -> PathBuf {
        self.runtime.join("channel_state")
    }

    fn users(&self) -> PathBuf {
        self.runtime.join("user_channel_state")
    }
}

#[async_trait]
impl StateLayout for RuntimeLayout {
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.channels().join(format!("{channel}.json")))
    }

    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.agents().join(channel.to_string()))
    }

    /// The states of the users in `channel`, one file each, so that they go with the channel's.
    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.users().join(channel.to_string()))
    }

    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>> {
        let mut channels = match fs::read_dir(self.agents()).await {
            Ok(channels) => channels,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(to_backend_error(err)),
        };
        let mut found = Vec::new();
        while let Some(channel) = channels.next_entry().await.map_err(to_backend_error)? {
            found.push(channel.path());
        }
        Ok(found)
    }

    /// In one directory, so that adding a file never races with removing a directory emptied.
    fn under_way(&self) -> PathBuf {
        self.runtime.join("under_way")
    }
}
