//! Where the documents and the states of the backend `home` lie (issue 554): the layouts its
//! `FileDocuments` and `JsonStates` are given, over its `Places`.

use crate::places::{file_name_of, Places, Shelf};
use async_trait::async_trait;
use caelumora_domain::channel::{Channel, ChannelId};
use caelumora_domain::scope::Scope;
use caelumora_domain::store::{StoreResult, Stored};
use caelumora_store::documents::Document;
use caelumora_store_json::{id_named, to_backend_error, DocumentLayout, StateLayout};
use futures::stream::BoxStream;
use std::fs;
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

/// Gives the directory `dir` of a document, named by its id, a link beside it named `name`, made
/// anew, and takes away the links to it of other names.
#[cfg(unix)]
fn label(dir: &Path, name: &str) -> io::Result<()> {
    use std::os::unix::fs::symlink;
    let (Some(family), Some(id)) = (dir.parent(), dir.file_name()) else { return Ok(()) };
    let link = family.join(file_name_of(name));
    if fs::read_link(&link).is_ok_and(|to| to == Path::new(id)) {
        return Ok(());
    }
    unlabel(dir, Some(&link))?;
    let made = family.join(format!(".{}.link", Uuid::new_v4()));
    symlink(id, &made)?;
    fs::rename(&made, &link).inspect_err(|_| {
        let _ = fs::remove_file(&made);
    })
}

/// Links are not made where there are none.
#[cfg(not(unix))]
fn label(_dir: &Path, _name: &str) -> io::Result<()> {
    Ok(())
}

/// Takes away the links beside `dir` to it, but `keep`.
fn unlabel(dir: &Path, keep: Option<&Path>) -> io::Result<()> {
    let (Some(family), Some(id)) = (dir.parent(), dir.file_name()) else { return Ok(()) };
    let entries = match fs::read_dir(family) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let path = entry?.path();
        let to_it = fs::read_link(&path).is_ok_and(|to| to == Path::new(id));
        if to_it && Some(path.as_path()) != keep {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// The documents of one kind where its `Shelf` and their scopes say, each with a link beside its
/// directory named by its name, so that a person finds it: one the backend never reads, so that one
/// out of date or missing does no harm.
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

/// Removes the directories below `dir` that hold nothing, and the links to what is gone, and then
/// `dir` if it holds nothing. What holds anything stays, so that only what was left empty goes,
/// also if another program fills or removes it meanwhile. Answers whether `dir` is gone.
fn remove_if_empty(dir: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err),
    };
    let mut empty = true;
    let mut links = Vec::new();
    for entry in entries {
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            links.push(entry.path());
        } else if !kind.is_dir() || !remove_if_empty(&entry.path())? {
            empty = false;
        }
    }
    // After the directories, so that a link to one just removed goes too.
    for link in links {
        match fs::metadata(&link) {
            Err(err) if err.kind() == ErrorKind::NotFound => match fs::remove_file(&link) {
                Err(err) if err.kind() != ErrorKind::NotFound => return Err(err),
                _ => {}
            },
            _ => empty = false,
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

    fn all(&self) -> BoxStream<'_, StoreResult<(Uuid, PathBuf)>> {
        self.places.all(&self.shelf)
    }

    fn lock(&self) -> PathBuf {
        self.lock.clone()
    }

    /// Removes the directory of the document, once nothing is left in it: not what lies in it and
    /// has not gone before it.
    /// Its links, too, once its directory is gone.
    async fn removed(&self, path: &Path) -> StoreResult<()> {
        let Some(dir) = path.parent().map(Path::to_path_buf) else { return Ok(()) };
        debug!("Tidying up {}", dir.display());
        task::spawn_blocking(move || match remove_if_empty(&dir)? {
            true => unlabel(&dir, None),
            false => Ok(()),
        })
        .await
        .map_err(to_backend_error)?
        .map_err(to_backend_error)
    }

    /// None for a document of a kind that lies beside another or without a name, for one of a
    /// user's or a group's scope, whose names share a directory with those of others, and for one
    /// named as an id is, which would be taken for the directory of that id.
    async fn written(&self, path: &Path, document: &Document) -> StoreResult<()> {
        let Some(name) = document.name.as_ref().filter(|name| {
            !self.shelf.lies_beside()
                && !matches!(name.scope, Scope::UserScope { .. } | Scope::GroupScope { .. })
                && id_named(name.as_str()).is_none()
        }) else {
            return Ok(());
        };
        let (Some(dir), name) = (path.parent().map(Path::to_path_buf), name.as_str().to_string())
        else {
            return Ok(());
        };
        task::spawn_blocking(move || label(&dir, &name))
            .await
            .map_err(to_backend_error)?
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
        Ok(self
            .0
            .directory_of_kind(Channel::KIND.name(), channel.into())
            .await?
            .join(STATE)
            .join("channel.json"))
    }

    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self
            .0
            .directory_of_kind(Channel::KIND.name(), channel.into())
            .await?
            .join(STATE)
            .join("agents"))
    }

    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self
            .0
            .directory_of_kind(Channel::KIND.name(), channel.into())
            .await?
            .join(STATE)
            .join("users"))
    }

    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>> {
        Ok(self.0.channels().await?.into_iter().map(|c| c.join(STATE).join("agents")).collect())
    }

    fn under_way(&self) -> PathBuf {
        self.0.under_way()
    }
}
