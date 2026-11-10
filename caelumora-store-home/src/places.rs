//! Where things lie below the data directory (issue 554), told from their ids and scopes: a kind's
//! documents are found by looking through the directories things can lie in, and what is found
//! is remembered.

use caelumora_domain::channel::{Channel, ChannelId};
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::scope::Scope;
use caelumora_domain::session::Session;
use caelumora_domain::store::{EntityKind, StoreResult, Stored};
use caelumora_domain::value::name::ScopedName;
use caelumora_store_json::to_backend_error;
use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tokio::fs;
use uuid::Uuid;

/// The directory a document of `kind` lies in, in whatever it lies in, by that document's id.
fn family_of(kind: &str) -> String {
    format!("{kind}s")
}

/// What the documents of one kind are kept by: each is the file `file` in a directory of its own,
/// named by its id, in a directory `family` of whatever it lies in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Shelf {
    family: String,
    file: String,
    /// The kind whose document of the same id it lies beside, whatever its scope says.
    beside: Option<String>,
}

impl Shelf {
    /// Where the documents of `kind` lie, by themselves.
    fn of(kind: &str) -> Self {
        Self { family: family_of(kind), file: format!("{kind}.json"), beside: None }
    }
}

/// Which directories things lie in, below `data_dir`. What is found by looking is remembered,
/// never stored, and looked for again once it is not there any more, so that what another program
/// wrote or removed is seen.
#[derive(Debug)]
pub(crate) struct Places {
    data_dir: PathBuf,
    /// The kinds that lie beside the document of their id of another kind.
    within: BTreeMap<String, String>,
    found: Mutex<HashMap<(Shelf, Uuid), PathBuf>>,
}

impl Places {
    /// The places below `data_dir`, where the kinds named in `within` lie beside the document of
    /// their id of the kind named with them.
    pub(crate) fn new(data_dir: PathBuf, within: BTreeMap<String, String>) -> Self {
        Self { data_dir, within, found: Mutex::default() }
    }

    /// The directory of the locks of the kinds.
    pub(crate) fn locks(&self) -> PathBuf {
        self.data_dir.join("locks")
    }

    /// The directory of the index of the work under way.
    pub(crate) fn under_way(&self) -> PathBuf {
        self.data_dir.join("under_way")
    }

    /// Where the documents of `kind` lie.
    pub(crate) fn shelf_of(&self, kind: EntityKind) -> Shelf {
        match self.within.get(kind.name()) {
            Some(beside) => Shelf {
                file: format!("{kind}.json"),
                beside: Some(beside.clone()),
                ..Shelf::of(beside)
            },
            None => Shelf::of(kind.name()),
        }
    }

    /// Every directory something can lie in, the data directory first, all named by an id.
    async fn holders(&self) -> StoreResult<Vec<PathBuf>> {
        let channels = family_of(Channel::KIND.name());
        let projects = family_of(Project::KIND.name());
        let mut holders = vec![self.data_dir.clone()];
        holders.extend(directories_of_ids(&self.data_dir.join(&projects)).await?.into_values());
        for session in directories_of_ids(&self.sessions()).await?.into_values() {
            holders.extend(directories_of_ids(&session.join(&channels)).await?.into_values());
            holders.extend(directories_of_ids(&session.join(&projects)).await?.into_values());
            holders.push(session);
        }
        Ok(holders)
    }

    fn sessions(&self) -> PathBuf {
        self.data_dir.join(family_of(Session::KIND.name()))
    }

    /// Where the document `id` on `shelf` lies, if there is one.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn find(&self, shelf: &Shelf, id: Uuid) -> StoreResult<Option<PathBuf>> {
        let key = (shelf.clone(), id);
        let known = self.found.lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned();
        if let Some(path) = known {
            if exists(&path).await? {
                return Ok(Some(path));
            }
        }
        for holder in self.holders().await? {
            let path = holder.join(&shelf.family).join(id.to_string()).join(&shelf.file);
            if exists(&path).await? {
                self.found.lock().unwrap_or_else(PoisonError::into_inner).insert(key, path.clone());
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Every document on `shelf`, each with its id and where it lies.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn all(&self, shelf: &Shelf) -> StoreResult<Vec<(Uuid, PathBuf)>> {
        let mut documents = Vec::new();
        for holder in self.holders().await? {
            for (id, dir) in directories_of_ids(&holder.join(&shelf.family)).await? {
                let path = dir.join(&shelf.file);
                if exists(&path).await? {
                    documents.push((id, path));
                }
            }
        }
        Ok(documents)
    }

    /// The directory of what lies in `scope`: the data directory for what no directory holds, as a
    /// user, a group and the global scope, whose documents say which is theirs.
    async fn holder_of(&self, scope: &Scope) -> StoreResult<PathBuf> {
        Ok(match scope {
            Scope::SessionScope { session_id } => self.sessions().join(session_id.to_string()),
            Scope::ChannelScope { session_id, channel_id } => self
                .sessions()
                .join(session_id.to_string())
                .join(family_of(Channel::KIND.name()))
                .join(channel_id.to_string()),
            Scope::ProjectScope { project_id } => self.project(*project_id).await?,
            Scope::UserScope { .. } | Scope::GroupScope { .. } | Scope::GlobalScope => {
                self.data_dir.clone()
            }
        })
    }

    /// Where the new document `id` on `shelf`, of the name `name`, is to lie: where its scope says,
    /// the data directory for a kind without names, or beside what it lies beside, once that is
    /// there.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn place(
        &self,
        shelf: &Shelf,
        id: Uuid,
        name: Option<&ScopedName>,
    ) -> StoreResult<PathBuf> {
        if let Some(beside) = &shelf.beside {
            if let Some(path) = self.find(&Shelf::of(beside), id).await? {
                return Ok(path.with_file_name(&shelf.file));
            }
        }
        let dir = match name.filter(|_| shelf.beside.is_none()) {
            Some(name) => self.holder_of(&name.scope).await?,
            None => self.data_dir.clone(),
        };
        Ok(dir.join(&shelf.family).join(id.to_string()).join(&shelf.file))
    }

    /// The directory of the channel `channel`: that of its document, or `channels/<id>` for a
    /// channel without one, whose session nothing tells.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn channel(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        self.directory_of(&Shelf::of(Channel::KIND.name()), channel.into()).await
    }

    /// The directory of the project `project`, as `channel` tells that of a channel.
    async fn project(&self, project: ProjectId) -> StoreResult<PathBuf> {
        self.directory_of(&Shelf::of(Project::KIND.name()), project.into()).await
    }

    /// The directory of the document `id` on `shelf`, or the one below the data directory it would
    /// have.
    async fn directory_of(&self, shelf: &Shelf, id: Uuid) -> StoreResult<PathBuf> {
        Ok(match self.find(shelf, id).await? {
            Some(path) => path.parent().map_or_else(|| path.clone(), Path::to_path_buf),
            None => self.data_dir.join(&shelf.family).join(id.to_string()),
        })
    }

    /// The directories of every channel, those of a session and those without a document.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn channels(&self) -> StoreResult<Vec<PathBuf>> {
        let channels = family_of(Channel::KIND.name());
        let mut all: Vec<PathBuf> =
            directories_of_ids(&self.data_dir.join(&channels)).await?.into_values().collect();
        for session in directories_of_ids(&self.sessions()).await?.into_values() {
            all.extend(directories_of_ids(&session.join(&channels)).await?.into_values());
        }
        Ok(all)
    }
}

/// Whether there is something at `path`.
async fn exists(path: &Path) -> StoreResult<bool> {
    fs::try_exists(path).await.map_err(to_backend_error)
}

/// The directories in `dir` named by an id, none if it is not there. Named as they are written -
/// hyphenated, in small letters -, so that what is listed is found by its id; a name beside the
/// directory of what has one, a copy, are passed over, and a link is not followed.
async fn directories_of_ids(dir: &Path) -> StoreResult<HashMap<Uuid, PathBuf>> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(to_backend_error(err)),
    };
    let mut directories = HashMap::new();
    while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| Uuid::parse_str(name).ok().filter(|id| id.to_string() == name))
        else {
            continue;
        };
        if entry.file_type().await.map_err(to_backend_error)?.is_dir() {
            directories.insert(id, entry.path());
        }
    }
    Ok(directories)
}
