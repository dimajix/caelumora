//! Where documents and states lie as files (issue 554): a backend over files says so with a
//! `DocumentLayout` and a `StateLayout`, and `FileDocuments` and `JsonStates` keep them there.

use super::to_backend_error;
use async_trait::async_trait;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::store::{EntityKind, StoreResult};
use caelumora_domain::value::name::ScopedName;
use caelumora_store::documents::Document;
use futures::stream::{self, BoxStream};
use futures::{Stream, StreamExt, TryStreamExt};
use std::fmt::Debug;
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::fs::{self, DirEntry, ReadDir};
use tracing::warn;
use uuid::Uuid;

/// Where the documents of one kind lie as files. A layout makes no directories: what is written
/// makes those it needs, and a directory not there holds nothing.
///
/// # Errors
/// A method answering a `StoreResult` fails if the place cannot be told, and what the storage was
/// asked then fails with that error, but where a method says otherwise.
#[async_trait]
pub trait DocumentLayout: Debug + Send + Sync {
    /// Where the document `id` lies, if there is one.
    async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>>;

    /// Where the new `document` is to lie: where nothing lies, whatever a file there would be.
    async fn place(&self, document: &Document) -> StoreResult<PathBuf>;

    /// Every document of the kind, each with its id and where it lies, as they are found.
    fn all(&self) -> BoxStream<'_, StoreResult<(Uuid, PathBuf)>>;

    /// The documents that can have the name `name`, as `all` tells them; all of them, unless a
    /// layout can tell.
    fn named<'a>(&'a self, _name: &'a ScopedName) -> BoxStream<'a, StoreResult<(Uuid, PathBuf)>> {
        self.all()
    }

    /// The file whose lock a write of the kind holds.
    fn lock(&self) -> PathBuf;

    /// Where `document`, now at `path`, is to lie: where nothing lies, or where it is, unless a
    /// layout says otherwise.
    async fn moved_to(&self, path: &Path, _document: &Document) -> StoreResult<PathBuf> {
        Ok(path.to_path_buf())
    }

    /// Told of the `document` just written at `path`; nothing unless a layout says otherwise. A
    /// failure is logged, the document being written all the same.
    async fn written(&self, _path: &Path, _document: &Document) -> StoreResult<()> {
        Ok(())
    }

    /// Tidies up once the file at `path` was removed; nothing unless a layout says otherwise. A
    /// failure is logged, the document being gone all the same.
    async fn removed(&self, _path: &Path) -> StoreResult<()> {
        Ok(())
    }
}

/// The layout of the documents of one kind in the JSON store: one file per document in the kind's
/// directory, `entities/<kind>`, named by its id, its lock beside that directory.
#[derive(Debug)]
pub(crate) struct EntitiesLayout {
    dir: PathBuf,
}

impl EntitiesLayout {
    /// The layout of the documents of `kind` under `root`.
    ///
    /// # Errors
    /// If its directory cannot be made, so that a path that cannot be written fails the start.
    pub(crate) fn new(root: &Path, kind: EntityKind) -> StoreResult<Self> {
        let dir = root.join("entities").join(kind.name());
        std_fs::create_dir_all(&dir).map_err(to_backend_error)?;
        Ok(Self { dir })
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

    fn all(&self) -> BoxStream<'_, StoreResult<(Uuid, PathBuf)>> {
        documents_in_dir(&self.dir).boxed()
    }

    fn lock(&self) -> PathBuf {
        self.dir.with_extension("lock")
    }
}

/// What is in the directory `dir`, as it is read; nothing if it is not there.
pub fn entries_in(dir: PathBuf) -> impl Stream<Item = StoreResult<DirEntry>> + Send {
    stream::try_unfold(None, move |entries: Option<ReadDir>| {
        let dir = dir.clone();
        async move {
            let mut entries = match entries {
                Some(entries) => entries,
                None => match fs::read_dir(&dir).await {
                    Ok(entries) => entries,
                    Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
                    Err(err) => return Err(to_backend_error(err)),
                },
            };
            let entry = entries.next_entry().await.map_err(to_backend_error)?;
            Ok(entry.map(|entry| (entry, Some(entries))))
        }
    })
}

/// The id `name` is, written as an id is written - hyphenated, in small letters -, so that what is
/// listed under it is found by that id.
pub fn id_named(name: &str) -> Option<Uuid> {
    Uuid::parse_str(name).ok().filter(|id| id.to_string() == name)
}

/// The documents in `dir` - every `*.json` file named by a UUID -, each with its id, the file's
/// name without `.json`, and its path, as the directory is read; temporary files of writes in
/// progress, or left behind by a crash, are not among them, and a file of another name is left out
/// and logged. Only the directory is read: a document is read when it is its turn.
fn documents_in_dir(dir: &Path) -> impl Stream<Item = StoreResult<(Uuid, PathBuf)>> + Send + '_ {
    entries_in(dir.to_path_buf()).try_filter_map(move |entry| async move {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            return Ok(None);
        }
        let id = path.file_stem().and_then(|stem| stem.to_str()).and_then(id_named);
        if id.is_none() {
            warn!("A document in {:?} is not named by its id: {:?}", dir, path.file_name());
        }
        Ok(id.map(|id| (id, path)))
    })
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
