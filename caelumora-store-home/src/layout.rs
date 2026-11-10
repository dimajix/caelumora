//! Where the documents and the states of the backend `home` lie (issue 554): the layouts its
//! `JsonDocuments` and `JsonStates` are given, over its `Places`.

use crate::places::{Places, Shelf};
use async_trait::async_trait;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::store::StoreResult;
use caelumora_store::documents::Document;
use caelumora_store_json::{to_backend_error, DocumentLayout, StateLayout};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task;
use tracing::debug;
use uuid::Uuid;

/// The directory in a channel's the states of the channel lie in, so that nothing that lies in the
/// channel's scope can be taken for them.
const STATE: &str = "state";

/// The documents of one kind where its `Shelf` and their scopes say, a lock of the file system
/// keeping apart the writers of every program.
#[derive(Debug)]
pub(crate) struct HomeDocuments {
    places: Arc<Places>,
    shelf: Shelf,
    /// The file whose lock a write holds.
    lock: PathBuf,
}

impl HomeDocuments {
    /// The layout of the documents on `shelf`, a write holding the lock of the file `lock`.
    pub(crate) fn new(places: Arc<Places>, shelf: Shelf, lock: PathBuf) -> Self {
        Self { places, shelf, lock }
    }
}

/// Opens the file at `path`, made if it is not there, and waits for its lock.
fn locked(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    file.lock()?;
    Ok(file)
}

/// Removes the directories below `dir` that hold nothing, and then `dir` if it holds nothing. What
/// holds anything stays, so that only what was left empty goes, also if another program fills or
/// removes it meanwhile. Answers whether `dir` is gone.
fn remove_if_empty(dir: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err),
    };
    let mut empty = true;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !remove_if_empty(&entry.path())? {
            empty = false;
        }
    }
    if !empty {
        return Ok(false);
    }
    match fs::remove_dir(dir) {
        Err(err) if err.kind() == ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
        _ => Ok(true),
    }
}

#[async_trait]
impl DocumentLayout for HomeDocuments {
    async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>> {
        self.places.find(&self.shelf, id).await
    }

    async fn place(&self, document: &Document) -> StoreResult<PathBuf> {
        self.places.place(&self.shelf, document.id, document.name.as_ref()).await
    }

    async fn all(&self) -> StoreResult<Vec<(Uuid, PathBuf)>> {
        self.places.all(&self.shelf).await
    }

    /// On a thread of its own, since the lock may be another program's, and held by the file it
    /// answers: closing it lets go.
    async fn writing(&self) -> StoreResult<Box<dyn Send>> {
        let lock = self.lock.clone();
        let file = task::spawn_blocking(move || locked(&lock))
            .await
            .map_err(to_backend_error)?
            .map_err(to_backend_error)?;
        Ok(Box::new(file))
    }

    /// Removes the directory of the document, once nothing is left in it: not what lies in it and
    /// has not gone before it.
    async fn removed(&self, path: &Path) -> StoreResult<()> {
        let Some(dir) = path.parent().map(Path::to_path_buf) else { return Ok(()) };
        debug!("Tidying up {}", dir.display());
        task::spawn_blocking(move || remove_if_empty(&dir))
            .await
            .map_err(to_backend_error)?
            .map(drop)
            .map_err(to_backend_error)
    }
}

/// The states of a channel in the directory `state` of the channel's own.
#[derive(Debug)]
pub(crate) struct HomeStates(Arc<Places>);

impl HomeStates {
    /// The layout of the states over `places`.
    pub(crate) fn new(places: Arc<Places>) -> Self {
        Self(places)
    }
}

#[async_trait]
impl StateLayout for HomeStates {
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.0.channel(channel).await?.join(STATE).join("channel.json"))
    }

    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.0.channel(channel).await?.join(STATE).join("agents"))
    }

    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.0.channel(channel).await?.join(STATE).join("users"))
    }

    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>> {
        Ok(self.0.channels().await?.into_iter().map(|c| c.join(STATE).join("agents")).collect())
    }

    fn under_way(&self) -> PathBuf {
        self.0.under_way()
    }
}
