//! Where things lie below the data and the configuration directory (issue 554), told from their
//! ids, names and scopes: a kind's documents are found by looking through the directories things
//! can lie in, and what is found is remembered.

use caelumora_domain::channel::Channel;
use caelumora_domain::group::Group;
use caelumora_domain::project::Project;
use caelumora_domain::scope::Scope;
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{EntityKind, StoreError, StoreResult, Stored};
use caelumora_domain::user::User;
use caelumora_domain::value::name::ScopedName;
use caelumora_store_json::{entries_in, id_named, to_backend_error};
use futures::stream::{self, BoxStream};
use futures::{Stream, StreamExt, TryStreamExt};
use std::collections::{BTreeMap, HashMap};
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

    /// Whether its documents lie beside those of another kind, in their directories.
    pub(crate) fn lies_beside(&self) -> bool {
        self.beside.is_some()
    }
}

/// The text a file or a link is named by for `name`: the name as it is, but for `%`, `/` and a
/// leading `.`, which are written `%25`, `%2F` and `%2E`, so that it can be told back.
pub(crate) fn file_name_of(name: &str) -> String {
    let escaped = name.replace('%', "%25").replace('/', "%2F");
    match escaped.strip_prefix('.') {
        Some(rest) => format!("%2E{rest}"),
        None => escaped,
    }
}

/// The name a file or a link is named by, as `file_name_of` wrote it; `None` for what it cannot
/// have written, as a hidden file.
pub(crate) fn name_of_file(text: &str) -> Option<String> {
    if text.starts_with('.') {
        return None;
    }
    let mut name = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        name.push_str(&rest[..at]);
        let escaped = rest.get(at..at + 3)?;
        name.push(match escaped {
            "%25" => '%',
            "%2F" => '/',
            "%2E" if at == 0 && name.is_empty() => '.',
            _ => return None,
        });
        rest = &rest[at + 3..];
    }
    name.push_str(rest);
    Some(name)
}

/// Which directories things lie in, below `data_dir`, and definitions of the global scope below
/// `config_dir`. What is found by looking is remembered, never stored, and looked for again once it
/// is not there any more, so that what another program wrote or removed is seen.
#[derive(Debug)]
pub(crate) struct Places {
    data_dir: PathBuf,
    /// Where the definitions of the global scope lie, if there are definitions.
    config_dir: Option<PathBuf>,
    /// The kinds that lie beside the document of their id of another kind.
    within: BTreeMap<String, String>,
    found: Mutex<HashMap<(Shelf, Uuid), PathBuf>>,
}

impl Places {
    /// The places below `data_dir` and `config_dir`, where the kinds named in `within` lie beside
    /// the document of their id of the kind named with them; without `config_dir`, of what keeps no
    /// definitions, as the content.
    pub(crate) fn new(
        data_dir: PathBuf,
        config_dir: Option<PathBuf>,
        within: BTreeMap<String, String>,
    ) -> Self {
        Self { data_dir, config_dir, within, found: Mutex::default() }
    }

    /// The data directory.
    pub(crate) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The directory of the locks of the kinds.
    pub(crate) fn locks(&self) -> PathBuf {
        self.data_dir.join("locks")
    }

    /// The file whose lock holds session `id` for the process it runs in (issue 554): beside the
    /// locks of the kinds, apart from the session's directory, which the lock would keep from being
    /// removed with the session.
    pub(crate) fn session_lock(&self, id: SessionId) -> PathBuf {
        self.locks().join("sessions").join(id.to_string())
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
        holders.extend(paths_of_ids(self.data_dir.join(&projects)).await?);
        for session in paths_of_ids(self.sessions()).await? {
            holders.extend(paths_of_ids(session.join(&channels)).await?);
            holders.extend(paths_of_ids(session.join(&projects)).await?);
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

    /// Every document on `shelf`, each with its id and where it lies, as the directories are read.
    pub(crate) fn all<'a>(
        &'a self,
        shelf: &'a Shelf,
    ) -> BoxStream<'a, StoreResult<(Uuid, PathBuf)>> {
        stream::once(self.holders())
            .map_ok(|holders| stream::iter(holders.into_iter().map(Ok::<_, StoreError>)))
            .try_flatten()
            .map_ok(|holder: PathBuf| directories_of_ids(holder.join(&shelf.family)))
            .try_flatten()
            .try_filter_map(move |(id, dir)| async move {
                let path = dir.join(&shelf.file);
                Ok(exists(&path).await?.then_some((id, path)))
            })
            .boxed()
    }

    /// Every directory a definition can lie in: the configuration directory, those of the data
    /// directory something lies in, and those of the users and the groups. Each holds the
    /// definitions of its scope (`scope_of`).
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn holders_of_definitions(&self) -> StoreResult<Vec<PathBuf>> {
        let mut holders: Vec<PathBuf> = self.config_dir.iter().cloned().collect();
        for holder in self.holders().await? {
            holders.extend(paths_of_ids(holder.join(family_of(User::KIND.name()))).await?);
            holders.extend(paths_of_ids(holder.join(family_of(Group::KIND.name()))).await?);
            if holder != self.data_dir {
                holders.push(holder);
            }
        }
        Ok(holders)
    }

    /// The scope of what lies in the directory `holder`, one `holders_of_definitions` answers.
    pub(crate) fn scope_of(&self, holder: &Path) -> Option<Scope> {
        if Some(holder) == self.config_dir.as_deref() {
            return Some(Scope::GlobalScope);
        }
        let parts: Vec<&str> = holder
            .strip_prefix(&self.data_dir)
            .ok()?
            .iter()
            .map(|part| part.to_str())
            .collect::<Option<_>>()?;
        let [.., family, id] = parts.as_slice() else { return None };
        let id = id_named(id)?;
        let is = |kind: EntityKind| *family == family_of(kind.name());
        Some(if is(Channel::KIND) {
            let [.., sessions, session, _, _] = parts.as_slice() else { return None };
            if *sessions != family_of(Session::KIND.name()) {
                return None;
            }
            Scope::ChannelScope { session_id: id_named(session)?.into(), channel_id: id.into() }
        } else if is(Session::KIND) {
            Scope::SessionScope { session_id: id.into() }
        } else if is(Project::KIND) {
            Scope::ProjectScope { project_id: id.into() }
        } else if is(User::KIND) {
            Scope::UserScope { user_id: id.into() }
        } else if is(Group::KIND) {
            Scope::GroupScope { group_id: id.into() }
        } else {
            return None;
        })
    }

    /// The directory of the definitions of `scope`: the configuration directory for the global
    /// one, that of its user or its group for theirs, where names of others' cannot take its
    /// files, and that of what holds it otherwise.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn holder_of_definitions(&self, scope: &Scope) -> StoreResult<PathBuf> {
        match scope {
            Scope::GlobalScope => self
                .config_dir
                .clone()
                .ok_or(StoreError::Backend("there is no directory of definitions".to_string())),
            Scope::UserScope { user_id } => {
                self.directory_of(&Shelf::of(User::KIND.name()), (*user_id).into()).await
            }
            Scope::GroupScope { group_id } => {
                self.directory_of(&Shelf::of(Group::KIND.name()), (*group_id).into()).await
            }
            Scope::SessionScope { .. }
            | Scope::ChannelScope { .. }
            | Scope::ProjectScope { .. } => self.holder_of(scope).await,
        }
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
            Scope::ProjectScope { project_id } => {
                self.directory_of_kind(Project::KIND.name(), (*project_id).into()).await?
            }
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

    /// The directory of the document `id` of the kind of the name `kind`: that of its document, or
    /// `<kind>s/<id>` below the data directory for one without, of which nothing tells where it
    /// would lie - the states of a channel no document names.
    ///
    /// # Errors
    /// If a directory cannot be looked into.
    pub(crate) async fn directory_of_kind(&self, kind: &str, id: Uuid) -> StoreResult<PathBuf> {
        self.directory_of(&Shelf::of(kind), id).await
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
        let mut all = paths_of_ids(self.data_dir.join(&channels)).await?;
        for session in paths_of_ids(self.sessions()).await? {
            all.extend(paths_of_ids(session.join(&channels)).await?);
        }
        Ok(all)
    }
}

/// Whether there is something at `path`.
async fn exists(path: &Path) -> StoreResult<bool> {
    fs::try_exists(path).await.map_err(to_backend_error)
}

/// The directories in `dir` named by an id, none if it is not there, as it is read. A name beside
/// the directory of what has one, a copy, are passed over, and a link is not followed.
fn directories_of_ids(dir: PathBuf) -> impl Stream<Item = StoreResult<(Uuid, PathBuf)>> + Send {
    entries_in(dir).try_filter_map(|entry| async move {
        let Some(id) = entry.file_name().to_str().and_then(id_named) else { return Ok(None) };
        let is_dir = entry.file_type().await.map_err(to_backend_error)?.is_dir();
        Ok(is_dir.then(|| (id, entry.path())))
    })
}

/// The directories in `dir` named by an id.
async fn paths_of_ids(dir: PathBuf) -> StoreResult<Vec<PathBuf>> {
    directories_of_ids(dir).map_ok(|(_, path)| path).try_collect().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_told_back_from_the_name_of_its_file() {
        for (name, file) in [
            ("plain", "plain"),
            ("a/b", "a%2Fb"),
            ("50%", "50%25"),
            (".hidden", "%2Ehidden"),
            ("a.b", "a.b"),
            ("Zoë", "Zoë"),
        ] {
            assert_eq!(file_name_of(name), file);
            assert_eq!(name_of_file(file).as_deref(), Some(name));
        }
        assert_eq!(name_of_file("a%2Eb"), None, "a dot is written so only at the start");
        assert_eq!(name_of_file("a%"), None);
        assert_eq!(name_of_file(".hidden"), None, "a hidden file is none it wrote");
    }
}
