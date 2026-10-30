//! The JSON store (issue 446): a backend of the stores that keeps every
//! document and every state in files below a directory (`JsonFileStore`),
//! registered as the backend `json` by its plugin (`JsonStorePlugin`).

mod chains;
mod histories;
pub mod plugin;
mod transcripts;

use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use caelumora_model::channel::{ChannelId, ChannelState};
use caelumora_model::security::Admission;
use caelumora_model::store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::query::{ordered_after, FieldPath, Filter};
use caelumora_model::store::transcript::{load_named_by_turn, TranscriptStore};
use caelumora_model::store::{
    EntityKind, Journaled, PersistenceBackend, Presence, Revision, StateStore, StoreError,
    StoreResult,
};
use caelumora_model::user::{UserChannelState, UserId};
use caelumora_model::value::name::ScopedName;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::{self, Debug, Display};
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// A journal is folded into its snapshot once it is at least as large as
/// the snapshot, and at least this large (issue 193): small states are not
/// written whole at every other put. `journal_floor` of the configuration.
pub const DEFAULT_JOURNAL_FLOOR: u64 = 64 * 1024;

/// The parameters of the backend `json` (issue 446): the directory the
/// files lie below, and the size from which a state's journal is folded
/// into its snapshot, `DEFAULT_JOURNAL_FLOOR` unless named.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonStoreConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub journal_floor: Option<u64>,
}

impl JsonStoreConfig {
    /// The store of the files this configuration names.
    ///
    /// # Errors
    /// As `JsonFileStore::new`.
    pub fn store(&self) -> StoreResult<JsonFileStore> {
        let store = JsonFileStore::new(&self.path)?;
        Ok(match self.journal_floor {
            Some(bytes) => store.with_journal_floor(bytes),
            None => store,
        })
    }
}

/// One JSON file per document (`EntityStore`), and per state a snapshot
/// and a journal (`StateStore`, issue 193): the state as it was last written
/// whole in `<key>.json`, and what changed since as one line per put in
/// `<key>.jsonl`. Loading reads the snapshot and replays the journal; a
/// put appends a line, or writes a new snapshot and empties the journal
/// once the journal is at least as large as the snapshot and
/// `journal_floor`. What a crash leaves is read as it is: a torn last line
/// is dropped, and a journal that survived its snapshot replays onto it
/// without effect, since replaying is idempotent. A state's history and an
/// agent's transcript are kept apart, in segments beside the snapshot
/// (`chains`; `histories`, `transcripts`, issues 379, 380).
#[derive(Debug)]
pub struct JsonFileStore {
    root: PathBuf,
    journal_floor: u64,
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<JsonDocuments>>>,
}

impl JsonFileStore {
    /// A store of the files under `root`.
    ///
    /// # Errors
    /// If the directories of the states cannot be made.
    pub fn new(root: impl Into<PathBuf>) -> StoreResult<Self> {
        let root = root.into();
        info!("Creating JSON file store at {:?}", root);
        let store =
            Self { journal_floor: DEFAULT_JOURNAL_FLOOR, documents: Default::default(), root };
        store.ensure_layout()?;
        Ok(store)
    }

    /// With a journal folded into its snapshot once it is `bytes` large, if
    /// that is more than the snapshot.
    pub fn with_journal_floor(mut self, bytes: u64) -> Self {
        self.journal_floor = bytes;
        self
    }

    fn ensure_layout(&self) -> StoreResult<()> {
        for dir in [
            self.agent_states_dir(),
            self.agent_channel_states_dir(),
            self.channel_states_dir(),
            self.user_channel_states_dir(),
            self.under_way_dir(),
        ] {
            std_fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(())
    }

    fn agent_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent_state")
    }

    fn agent_channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent_channel_state")
    }

    fn channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("channel_state")
    }

    fn agent_channel_state_path(&self, agent_id: AgentId, channel_id: ChannelId) -> PathBuf {
        self.agent_channel_state_dir(channel_id).join(format!("{}.json", agent_id))
    }

    fn agent_channel_state_dir(&self, channel_id: ChannelId) -> PathBuf {
        self.agent_channel_states_dir().join(channel_id.to_string())
    }

    fn user_channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("user_channel_state")
    }

    /// The states of the users in `channel_id`, one file each, so that they
    /// go with the channel's.
    fn user_channel_state_dir(&self, channel_id: ChannelId) -> PathBuf {
        self.user_channel_states_dir().join(channel_id.to_string())
    }

    fn user_channel_state_path(&self, user_id: UserId, channel_id: ChannelId) -> PathBuf {
        self.user_channel_state_dir(channel_id).join(format!("{user_id}.json"))
    }

    /// The index of the work under way (issue 388): an empty file
    /// `<channel_id>.<agent_id>` for each agent whose state in that channel
    /// has work under way. In one directory, so that adding a file never
    /// races with removing a directory emptied.
    fn under_way_dir(&self) -> PathBuf {
        self.root.join("runtime").join("under_way")
    }

    fn under_way_path(&self, agent_id: AgentId, channel_id: ChannelId) -> PathBuf {
        self.under_way_dir().join(format!("{channel_id}.{agent_id}"))
    }

    fn agent_state_path(&self, id: AgentId) -> PathBuf {
        self.agent_states_dir().join(format!("{id}.json"))
    }

    fn channel_state_path(&self, id: ChannelId) -> PathBuf {
        self.channel_states_dir().join(format!("{id}.json"))
    }
}

/// Writes `value` to `path` atomically: into a temporary file beside it,
/// then renamed over it. The temporary name is unique per write, so that two
/// writers of the same file do not share one (issue 62): each rename
/// succeeds, and the last one wins.
async fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> StoreResult<()> {
    let payload = serde_json::to_vec_pretty(value).map_err(to_backend_error)?;
    let mut tmp_name: OsString = path
        .file_name()
        .ok_or_else(|| StoreError::Backend(format!("invalid path: {}", path.display())))?
        .to_os_string();
    tmp_name.push(format!(".{}.tmp", Uuid::new_v4()));
    let tmp_path = path.with_file_name(tmp_name);
    if let Err(err) = fs::write(&tmp_path, payload).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    if let Err(err) = fs::rename(&tmp_path, path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    Ok(())
}

async fn read_json_opt<T: DeserializeOwned>(path: &Path) -> StoreResult<Option<T>> {
    match fs::read_to_string(path).await {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(to_backend_error)?;
            Ok(Some(parsed))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// What the file at `path` holds, `None` if it is gone or holds no JSON -
/// which is logged: one broken file does not break a list.
///
/// # Errors
/// If the file cannot be read.
async fn read_body(path: &Path) -> StoreResult<Option<Value>> {
    let text = match fs::read_to_string(path).await {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(to_backend_error(err)),
    };
    match serde_json::from_str(&text) {
        Ok(body) => Ok(Some(body)),
        Err(err) => {
            warn!("Failed to read JSON file {:?}: {}", path, err);
            Ok(None)
        }
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

        // Named as `JsonDocuments::path` names it - hyphenated, in small
        // letters -, so that what a scan lists is found by its id.
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        match Uuid::parse_str(stem) {
            Ok(id) if id.to_string() == stem => documents.push((id, path)),
            _ => warn!("A document in {:?} is not named by its id: {:?}", dir, path.file_name()),
        }
    }
    Ok(documents)
}

async fn delete_if_exists(path: &Path) -> StoreResult<bool> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// Creates the empty file `path` unless it exists.
async fn create_if_missing(path: &Path) -> StoreResult<()> {
    match OpenOptions::new().write(true).create_new(true).open(path).await {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(to_backend_error(err)),
    }
}

async fn delete_dir_if_exists(path: &Path) -> StoreResult<bool> {
    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// The journal of the state whose snapshot is at `path`: `<key>.jsonl`.
fn journal_path(path: &Path) -> PathBuf {
    path.with_extension("jsonl")
}

/// The size of the file at `path`, `None` if there is none.
async fn size_of(path: &Path) -> StoreResult<Option<u64>> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// The state whose snapshot is at `path`, with its journal replayed onto
/// it; `None` if there is no snapshot. A last line the journal does not end
/// with a newline and that does not parse is what a crash during an append
/// left, and is dropped; any other line that does not parse, or does not
/// fit the state, is a corrupt state.
async fn load_state<T: Journaled>(path: &Path) -> StoreResult<Option<T>> {
    let Some(mut state) = read_json_opt::<T>(path).await? else { return Ok(None) };
    let journal = journal_path(path);
    let text = match fs::read_to_string(&journal).await {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Some(state)),
        Err(err) => return Err(to_backend_error(err)),
    };
    let torn = !text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let changes: Vec<T::Change> = match serde_json::from_str(line) {
            Ok(changes) => changes,
            Err(err) if torn && i + 1 == lines.len() => {
                warn!(
                    "The journal {} ends in a torn line, left by a crash; it is dropped: {}",
                    journal.display(),
                    err
                );
                break;
            }
            Err(err) => {
                return Err(StoreError::Backend(format!(
                    "line {} of the journal of a stored state is corrupt: {err}",
                    i + 1
                )))
            }
        };
        for change in changes {
            state.apply(change).map_err(|e| {
                StoreError::Backend(format!(
                    "line {} of the journal does not fit the stored state: {e}",
                    i + 1
                ))
            })?;
        }
    }
    Ok(Some(state))
}

/// Appends `changes` as one line to the journal at `journal`, which is
/// `len_before` bytes long. Should the write fail, the journal is cut back
/// to what it was, so that a torn line is never followed by a good one.
async fn append_changes<C: Serialize>(
    journal: &Path,
    len_before: u64,
    changes: &[C],
) -> StoreResult<()> {
    let mut line = serde_json::to_vec(changes).map_err(to_backend_error)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(journal)
        .await
        .map_err(to_backend_error)?;
    // A tokio file hands the write to a blocking task: flushed, it is done
    // before the put returns, and the next put sees the journal's size.
    let written = match file.write_all(&line).await {
        Ok(()) => file.flush().await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = file.set_len(len_before).await;
        return Err(to_backend_error(err));
    }
    Ok(())
}

fn to_backend_error(err: impl Display) -> StoreError {
    StoreError::Backend(err.to_string())
}

/// Opens a kind in the directory of its name, `entities/<kind>`. The store of
/// the states is the JSON file store itself.
impl PersistenceBackend for JsonFileStore {
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        let mut all = self.documents.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(storage) = all.get(&kind) {
            return Ok(Arc::<JsonDocuments>::clone(storage));
        }
        let storage = Arc::new(JsonDocuments::new(&self.root, kind, document_of)?);
        all.insert(kind, Arc::clone(&storage));
        Ok(storage)
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Ok(self)
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Ok(self)
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Ok(self)
    }
}

/// The documents of one kind as JSON files (issue 446), one per document in
/// the kind's directory, `entities/<kind>`, named by its id. Every write
/// holds the kind's lock from reading what it judges - the revision it
/// writes in place of (issue 492), the names of the others (issue 250) - to
/// writing, so that no two writes miss each other and no two take one name.
struct JsonDocuments {
    kind: EntityKind,
    dir: PathBuf,
    /// What a document read is told by: its id, its name, its mark, its
    /// security attributes, its revision.
    document_of: DocumentOf,
    /// Held by every write: within this process, which is the only one that
    /// writes the files.
    writes: Mutex<()>,
}

impl Debug for JsonDocuments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonDocuments")
            .field("kind", &self.kind)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl JsonDocuments {
    /// The storage of the documents of `kind` under `root`.
    ///
    /// # Errors
    /// If its directory cannot be made.
    fn new(root: &Path, kind: EntityKind, document_of: DocumentOf) -> StoreResult<Self> {
        let dir = root.join("entities").join(kind.name());
        std_fs::create_dir_all(&dir).map_err(to_backend_error)?;
        Ok(Self { kind, dir, document_of, writes: Mutex::new(()) })
    }

    /// Where the document of `id` is.
    fn path(&self, id: Uuid) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The document at `path`; `None` if there is none, or it cannot be read
    /// or told, which is logged.
    async fn read(&self, path: &Path) -> Option<Document> {
        let body = match read_json_opt::<Value>(path).await {
            Ok(body) => body?,
            Err(err) => {
                warn!("Failed to read JSON file {:?}: {}", path, err);
                return None;
            }
        };
        match (self.document_of)(body) {
            Ok(document) => Some(document),
            Err(err) => {
                warn!("Failed to read the document {:?}: {}", path, err);
                None
            }
        }
    }

    /// The document `id` as stored, if it is at the revision `read`:
    /// `StoreError::Outdated` otherwise, also if there is none (issue 492).
    /// Called under the kind's lock.
    async fn at_revision(&self, id: Uuid, read: Revision) -> StoreResult<Document> {
        match self.read(&self.path(id)).await {
            Some(stored) if stored.revision == read => Ok(stored),
            _ => Err(StoreError::Outdated { entity: self.kind.name(), id: id.to_string() }),
        }
    }

    /// Refuses `document` if another document of the kind that is not
    /// deleted has its name (issue 250). Called under the kind's lock.
    async fn name_free(&self, document: &Document) -> StoreResult<()> {
        let Some(name) = named(document) else { return Ok(()) };
        for (_, path) in documents_in_dir(&self.dir).await? {
            if let Some(other) = self.read(&path).await {
                if other.id != document.id && named(&other) == Some(name) {
                    return Err(StoreError::NameTaken { entity: self.kind.name() });
                }
            }
        }
        Ok(())
    }
}

/// The name `document` takes: its name, unless it is deleted.
fn named(document: &Document) -> Option<&ScopedName> {
    document.name.as_ref().filter(|_| document.removal_id.is_none())
}

#[async_trait]
impl DocumentStorage for JsonDocuments {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        read_json_opt(&self.path(id)).await
    }

    /// Reads the files in the order of their names, each only when it is its
    /// turn, and judges `filter` on what they hold; whether one is marked
    /// deleted, and whether it is admitted, it leaves to the store, which
    /// would read it as its type to tell. A file that is not JSON is left
    /// out and logged.
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        _admission: Option<&Admission>,
        _presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        let mut found = Vec::new();
        for (id, path) in ordered_after(documents_in_dir(&self.dir).await?, after) {
            if found.len() >= limit {
                break;
            }
            let Some(body) = read_body(&path).await? else { continue };
            if filter.is_none_or(|filter| filter.matches(&body)) {
                found.push((id, body));
            }
        }
        Ok(found)
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        for (_, path) in documents_in_dir(&self.dir).await? {
            if let Some(document) = self.read(&path).await {
                if named(&document) == Some(name) {
                    return Ok(Some(document.body));
                }
            }
        }
        Ok(None)
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        let path = self.path(document.id);
        let _writing = self.writes.lock().await;
        if fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::Conflict(format!(
                "{} {} already exists",
                self.kind, document.id
            )));
        }
        self.name_free(&document).await?;
        debug!("Writing {} {} to {}", self.kind, document.id, path.display());
        write_json_atomic(&path, &document.body).await
    }

    /// Reads the others of the kind only if `document` takes another name
    /// than it has as stored: whatever else changes takes nobody's name.
    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        let path = self.path(document.id);
        let _writing = self.writes.lock().await;
        let stored = self.at_revision(document.id, read).await?;
        if named(&document).is_some() && named(&stored) != named(&document) {
            self.name_free(&document).await?;
        }
        debug!("Writing {} {} to {}", self.kind, document.id, path.display());
        write_json_atomic(&path, &document.body).await
    }

    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        let path = self.path(id);
        let _writing = self.writes.lock().await;
        if !fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Ok(false);
        }
        self.at_revision(id, read).await?;
        debug!("Purging {} at {}", self.kind, path.display());
        delete_if_exists(&path).await
    }
}

/// The state operations: a snapshot and a journal per state (issue 193).
impl JsonFileStore {
    /// Takes out of the index of the work under way every entry whose
    /// channel and agent, as written in its name, `matches`.
    async fn leave_under_way(&self, matches: impl Fn(&str, &str) -> bool) -> StoreResult<()> {
        let mut entries = match fs::read_dir(self.under_way_dir()).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(to_backend_error(err)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|name| name.split_once('.'))
                .is_some_and(|(channel, agent)| matches(channel, agent))
            {
                delete_if_exists(&entry.path()).await?;
            }
        }
        Ok(())
    }

    /// Stores `state` at `path`, and marks it stored: as a line of its
    /// journal, or - if there is no snapshot yet, or the journal is as large
    /// as the snapshot and `journal_floor` - as a new snapshot, after which
    /// the journal is emptied. The sizes are asked of the file system every
    /// time; nothing is kept between puts.
    async fn store_state<T: Journaled>(&self, path: &Path, state: &mut T) -> StoreResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(to_backend_error)?;
        }
        let journal = journal_path(path);
        let journal_len = size_of(&journal).await?.unwrap_or(0);
        match size_of(path).await? {
            Some(snapshot_len) if journal_len < snapshot_len.max(self.journal_floor) => {
                debug!("Appending to the journal {}", journal.display());
                append_changes(&journal, journal_len, &state.changes()).await?;
            }
            _ => {
                debug!("Writing the snapshot {}", path.display());
                write_json_atomic(path, state).await?;
                delete_if_exists(&journal).await?;
            }
        }
        state.mark_stored();
        Ok(())
    }
}

#[async_trait]
impl StateStore for JsonFileStore {
    async fn get_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
    ) -> StoreResult<AgentChannelState> {
        let path = self.agent_channel_state_path(agent_id, channel_id);
        debug!("Reading agent channel state from {}", path.display());
        let mut state: AgentChannelState = load_state(&path).await?.unwrap_or_default();
        state.transcript =
            self.load_transcript(agent_id, channel_id, state.transcript_from).await?;
        load_named_by_turn(self, agent_id, channel_id, &mut state).await?;
        Ok(state)
    }

    /// The prompt texts and the tool sets first, which what follows names,
    /// then the transcript, then the document, which says from where it is
    /// loaded. The index of the work under way is entered before the
    /// document and left after it: what a crash leaves between the two is a
    /// session started for nothing, never work that is not started.
    async fn put_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
        state: &mut AgentChannelState,
    ) -> StoreResult<()> {
        self.append_given(agent_id, channel_id, state).await?;
        self.append_transcript(agent_id, channel_id, &mut state.transcript).await?;
        let under_way = self.under_way_path(agent_id, channel_id);
        if state.has_work_under_way() {
            create_if_missing(&under_way).await?;
            self.store_state(&self.agent_channel_state_path(agent_id, channel_id), state).await
        } else {
            self.store_state(&self.agent_channel_state_path(agent_id, channel_id), state).await?;
            delete_if_exists(&under_way).await.map(drop)
        }
    }

    async fn channels_with_work_under_way(&self) -> StoreResult<Vec<ChannelId>> {
        let mut entries = match fs::read_dir(self.under_way_dir()).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(to_backend_error(err)),
        };
        let mut channels = HashSet::new();
        while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
            let name = entry.file_name();
            match name
                .to_str()
                .and_then(|name| name.split_once('.'))
                .and_then(|(channel, _)| channel.parse::<ChannelId>().ok())
            {
                Some(channel_id) => {
                    channels.insert(channel_id);
                }
                None => warn!(
                    "The index of the work under way holds a file that names no channel: {}",
                    name.to_string_lossy()
                ),
            }
        }
        Ok(channels.into_iter().collect())
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let path = self.agent_state_path(agent_id);
        debug!("Reading agent state from {}", path.display());
        Ok(load_state(&path).await?.unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        self.store_state(&self.agent_state_path(agent_id), state).await
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let path = self.channel_state_path(channel_id);
        debug!("Reading channel state from {}", path.display());
        let mut state: ChannelState = load_state(&path)
            .await?
            .ok_or(StoreError::NotFound { entity: "channel_state", id: channel_id.to_string() })?;
        state.history = self.load_history(channel_id, state.history_from).await?;
        Ok(state)
    }

    /// The history first, then the document, which says from where it is
    /// loaded.
    async fn put_channel_state(
        &self,
        channel_id: ChannelId,
        state: &mut ChannelState,
    ) -> StoreResult<()> {
        self.append_history(channel_id, &mut state.history).await?;
        self.store_state(&self.channel_state_path(channel_id), state).await
    }

    async fn get_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
    ) -> StoreResult<UserChannelState> {
        let path = self.user_channel_state_path(user_id, channel_id);
        debug!("Reading user channel state from {}", path.display());
        Ok(load_state(&path).await?.unwrap_or_default())
    }

    async fn put_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
        state: &mut UserChannelState,
    ) -> StoreResult<()> {
        self.store_state(&self.user_channel_state_path(user_id, channel_id), state).await
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        debug!("Deleting the state of channel {}", channel_id);
        let path = self.channel_state_path(channel_id);
        delete_if_exists(&path).await?;
        delete_if_exists(&journal_path(&path)).await?;
        delete_dir_if_exists(&self.history_dir(channel_id)).await?;
        delete_dir_if_exists(&self.agent_channel_state_dir(channel_id)).await?;
        delete_dir_if_exists(&self.user_channel_state_dir(channel_id)).await?;
        let channel_id = channel_id.to_string();
        self.leave_under_way(|channel, _| channel == channel_id).await
    }

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        delete_if_exists(&path).await?;
        delete_if_exists(&journal_path(&path)).await?;
        let mut channels = match fs::read_dir(self.agent_channel_states_dir()).await {
            Ok(channels) => channels,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(to_backend_error(err)),
        };
        while let Some(channel) = channels.next_entry().await.map_err(to_backend_error)? {
            let path = channel.path().join(format!("{agent_id}.json"));
            delete_if_exists(&path).await?;
            delete_if_exists(&journal_path(&path)).await?;
            delete_dir_if_exists(&path.with_extension("history")).await?;
            delete_dir_if_exists(&path.with_extension("transcript")).await?;
            delete_dir_if_exists(&path.with_extension("given")).await?;
        }
        let agent_id = agent_id.to_string();
        self.leave_under_way(|_, agent| agent == agent_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorState;
    use caelumora_model::agent::testing::make_personality;
    use caelumora_model::chain::Position;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::history::Reader;
    use caelumora_model::message::testing::{id, make_message};
    use caelumora_model::note::{NoteChange, NoteId};
    use caelumora_model::removal::Removal;
    use caelumora_model::security::{OwnershipPolicy, Policies, SecurityContext};
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::{Session, SessionId};
    use caelumora_model::store::query::Query;
    use caelumora_model::store::{
        suite, EntityStore, EntityStores, Persistence, StoreError, Stored,
    };
    use caelumora_model::turn::TurnState;
    use caelumora_model::value::name::Name;
    use caelumora_model::value::text::Description;
    use tempfile::tempdir;
    use tokio::task::JoinSet;

    fn store(dir: &Path) -> JsonFileStore {
        JsonFileStore::new(dir).unwrap()
    }

    /// The stores of the files under `dir`.
    fn files(dir: &Path) -> Persistence {
        Persistence::over(Arc::new(store(dir))).unwrap()
    }

    /// A new session of `name`, of the system, with nothing in it.
    fn session(name: &str) -> Session {
        Session { name: Name::new(name).unwrap(), ..bare_session(SessionId::new()) }
    }

    /// The store of the kind `T` in the files under `dir`, which permits
    /// everything.
    fn of_kind<T: Stored>(dir: &Path) -> Arc<dyn EntityStore<T>> {
        files(dir).entity_store(&Policies::permitting_all()).unwrap()
    }

    /// Where the document `id` of the kind `T` lies in the files under
    /// `dir`.
    fn path_of<T: Stored>(dir: &Path, id: &T::Id) -> PathBuf {
        dir.join("entities").join(T::KIND.name()).join(format!("{id}.json"))
    }

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        let dir = tempdir().unwrap();
        suite::entity_store_behaves(
            &EntityStores::open(&files(dir.path()), &Policies::permitting_all()).unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn lists_what_check_decides() {
        let dir = tempdir().unwrap();
        let (memberships, access_policy) = suite::admitting_policy();
        let store = files(dir.path());
        let policies = Policies::by_default_rules(Arc::clone(&access_policy));
        let folders = store.entity_store::<suite::Folder>(&policies).unwrap();
        suite::lists_admit_what_check_decides(
            &EntityStores::open(&store, &policies).unwrap(),
            folders.as_ref(),
            &memberships,
            access_policy.as_ref(),
        )
        .await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let dir = tempdir().unwrap();
        let store = files(dir.path());
        suite::queries_behave(&EntityStores::open(&store, &Policies::permitting_all()).unwrap())
            .await;
        suite::queries_reach_parts(
            store.entity_store::<suite::Folder>(&Policies::permitting_all()).unwrap().as_ref(),
        )
        .await;
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        let dir = tempdir().unwrap();
        suite::state_store_behaves(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn keeps_the_index_of_the_work_under_way() {
        let dir = tempdir().unwrap();
        suite::work_under_way_is_indexed(&store(dir.path())).await;

        // Read from the files: a store opened anew over them, as after a
        // restart, finds what was under way.
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut planning = AgentChannelState {
            processing_state: ActorState::Planning,
            turn: Some(TurnState::default()),
            ..AgentChannelState::default()
        };
        store(dir.path()).put_agent_channel_state(agent, channel, &mut planning).await.unwrap();
        assert_eq!(store(dir.path()).channels_with_work_under_way().await.unwrap(), [channel]);
    }

    #[tokio::test]
    async fn keeps_the_histories_of_the_states() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        suite::history_store_behaves(&store, &store).await;
        suite::transcript_store_behaves(&store, &store).await;
        suite::given_store_behaves(&store, &store).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        let dir = tempdir().unwrap();
        suite::deleting_marks(
            &EntityStores::open(&files(dir.path()), &Policies::permitting_all()).unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn judges_writes_on_their_revision() {
        let dir = tempdir().unwrap();
        suite::writes_are_judged_on_their_revision(
            &EntityStores::open(&files(dir.path()), &Policies::permitting_all()).unwrap(),
        )
        .await;
        suite::storage_judges_writes_on_their_revision(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn begins_purges() {
        let dir = tempdir().unwrap();
        suite::purges_are_begun(
            &EntityStores::open(&files(dir.path()), &Policies::permitting_all()).unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        let dir = tempdir().unwrap();
        suite::names_are_unique(
            &EntityStores::open(&files(dir.path()), &Policies::permitting_all()).unwrap(),
        )
        .await;
        let dir = tempdir().unwrap();
        suite::names_are_unique_beyond_what_is_read(
            &EntityStores::open(
                &files(dir.path()),
                &Policies::by_default_rules(Arc::new(OwnershipPolicy::new())),
            )
            .unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn a_document_stored_before_marks_existed_loads_and_one_not_deleted_is_written_without() {
        let dir = tempdir().unwrap();
        let sessions = of_kind::<Session>(dir.path());
        let system = SecurityContext::system();
        let mut session = session("old");
        sessions.create(&system, &mut session).await.unwrap();
        let text = fs::read_to_string(path_of::<Session>(dir.path(), &session.id)).await.unwrap();
        assert!(!text.contains("deleted"), "{text}");
        assert!(sessions.get(&system, session.id).await.unwrap().deleted.is_none());
    }

    /// A file in the directory of a kind that is not named by a UUID is no
    /// document of it (issue 455): a list leaves it out, and the rest is
    /// listed.
    #[tokio::test]
    async fn a_file_not_named_by_an_id_is_left_out_of_a_list() {
        let dir = tempdir().unwrap();
        let sessions = of_kind::<Session>(dir.path());
        let system = SecurityContext::system();
        let mut session = session("kept");
        sessions.create(&system, &mut session).await.unwrap();
        let stray = path_of::<Session>(dir.path(), &session.id).with_file_name("notes.json");
        fs::copy(path_of::<Session>(dir.path(), &session.id), &stray).await.unwrap();

        let listed = sessions.list(&system, &Query::all()).await.unwrap();
        assert_eq!(
            listed.items.iter().map(|s| s.id).collect::<Vec<_>>(),
            [session.id],
            "the stray file is no session"
        );
    }

    #[tokio::test]
    async fn concurrent_writes_of_one_file_all_succeed_and_leave_no_temporary_files() {
        // Entities may be written at the same time - by two requests that
        // change one (issue 62); a state has one writer (issue 215).
        use caelumora_model::agent::Personality;
        let dir = tempdir().unwrap();
        let personalities = of_kind::<Personality>(dir.path());
        let system = SecurityContext::system();
        let mut personality = make_personality("plain");
        personalities.create(&system, &mut personality).await.unwrap();

        let mut writes = JoinSet::new();
        for i in 0..16 {
            let (personalities, system) = (Arc::clone(&personalities), system.clone());
            let id = personality.id;
            // Each writer in place of what was written since (issue 492).
            writes.spawn(async move {
                loop {
                    let mut changed = personalities.get(&system, id).await?;
                    changed.description = Some(Description::new(format!("version {i}")).unwrap());
                    match personalities.update(&system, &mut changed).await {
                        Err(StoreError::Outdated { .. }) => continue,
                        written => return written,
                    }
                }
            });
        }
        while let Some(result) = writes.join_next().await {
            result.unwrap().expect("no writer loses its temporary file to another");
        }

        let path = path_of::<Personality>(dir.path(), &personality.id);
        let names: Vec<String> = std_fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [path.file_name().unwrap().to_string_lossy().into_owned()],
            "only the file itself"
        );
        let stored = personalities.get(&system, personality.id).await.expect("readable");
        assert!(
            stored
                .description
                .as_ref()
                .map(|d| d.as_str())
                .unwrap_or_default()
                .starts_with("version "),
            "one of the writes, whole"
        );
    }

    #[tokio::test]
    async fn of_concurrent_writes_under_one_name_one_takes_it() {
        // Checked and written under the kind's lock (issue 250): nothing
        // writes between another's check and its write.
        use caelumora_model::actor::ActorId;
        use caelumora_model::agent::Agent;
        let dir = tempdir().unwrap();
        let agents = of_kind::<Agent>(dir.path());
        let system = SecurityContext::system();
        let personality = make_personality("plain");

        let mut writes = JoinSet::new();
        for _ in 0..16 {
            let (agents, system) = (Arc::clone(&agents), system.clone());
            let mut agent = Agent::new(
                ScopedName::new("Helper", Default::default()).unwrap(),
                personality.id,
                ActorId::User(UserId::new()),
            );
            writes.spawn(async move { agents.create(&system, &mut agent).await });
        }
        let mut made = 0;
        while let Some(result) = writes.join_next().await {
            match result.unwrap() {
                Ok(()) => made += 1,
                Err(e) => assert!(
                    matches!(e, StoreError::NameTaken { entity: "agent" }),
                    "refused for its name: {e:?}"
                ),
            }
        }
        assert_eq!(made, 1);
        assert_eq!(
            agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn deleting_a_session_marks_its_file_and_purging_removes_it_leaving_its_state_to_the_state_store(
    ) {
        use caelumora_model::channel::testing::make_channel;

        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let sessions = of_kind::<Session>(dir.path());
        let ctx = SecurityContext::default();

        let session_id = SessionId(Uuid::new_v4());
        let channel = make_channel(session_id, "general");
        let channel_id = channel.id;
        let mut session =
            Session { id: session_id, channels: vec![channel.id], ..session("to-delete") };
        sessions.create(&ctx, &mut session).await.unwrap();

        // Write out channel state so we can assert it gets removed.
        let agent_id = AgentId(Uuid::new_v4());
        store
            .put_agent_channel_state(agent_id, channel_id, &mut AgentChannelState::default())
            .await
            .unwrap();

        assert!(path_of::<Session>(dir.path(), &session_id).exists());
        assert!(store.agent_channel_state_dir(channel_id).exists());

        assert!(sessions.delete(&ctx, session_id, &Removal::now(ctx.caller_id)).await.unwrap());
        assert!(path_of::<Session>(dir.path(), &session_id).exists(), "deleting marks the file");
        assert!(
            store.agent_channel_state_dir(channel_id).exists(),
            "the state is the state store's"
        );
        assert!(sessions.purge(&ctx, session_id).await.unwrap());
        assert!(!path_of::<Session>(dir.path(), &session_id).exists(), "purging removes it");

        store.put_channel_state(channel_id, &mut ChannelState::new(channel_id)).await.unwrap();
        store.delete_channel_state(channel_id).await.unwrap();
        assert!(!store.channel_state_path(channel_id).exists());
        assert!(!store.agent_channel_state_dir(channel_id).exists());
        store.delete_channel_state(channel_id).await.expect("nothing to delete is no error");

        // Deleting or purging again reports that nothing was there.
        assert!(!sessions.delete(&ctx, session_id, &Removal::now(ctx.caller_id)).await.unwrap());
        assert!(!sessions.purge(&ctx, session_id).await.unwrap());
        let err = sessions.get(&ctx, session_id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "session", .. }));
    }

    #[tokio::test]
    async fn list_sessions_skips_corrupted_files() {
        let dir = tempdir().unwrap();
        let sessions = of_kind::<Session>(dir.path());
        let ctx = SecurityContext::default();

        let mut s1 = session("session-1");
        sessions.create(&ctx, &mut s1).await.unwrap();

        // Create a corrupted JSON file
        let corrupted_path = dir.path().join(format!("entities/session/{}.json", Uuid::new_v4()));
        std_fs::write(corrupted_path, "invalid json").unwrap();

        // This should NOT fail, it should return s1 and log a warning
        let sessions = sessions.list(&ctx, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, s1.id);
    }

    /// The lines of the journal of the agent channel state of `agent` in
    /// `channel`, `None` if there is no journal.
    fn journal_lines(store: &JsonFileStore, agent: AgentId, channel: ChannelId) -> Option<usize> {
        std_fs::read_to_string(journal_path(&store.agent_channel_state_path(agent, channel)))
            .ok()
            .map(|text| text.lines().count())
    }

    fn journal_file(store: &JsonFileStore, agent: AgentId, channel: ChannelId) -> PathBuf {
        journal_path(&store.agent_channel_state_path(agent, channel))
    }

    /// A state with messages `from..to` in its transcript, taken in as an
    /// agent takes them in from its channel's history - moving where it
    /// stands in it (issue 440) -, and a note published for each in its
    /// document, which grows with them.
    fn said(state: &mut AgentChannelState, from: u128, to: u128) {
        for n in from..to {
            state.transcript.push(make_message(id(n)));
            state.last_message = Some(id(n));
            state.heard.took(Position::new(n as u64));
            let note = NoteChange::Add {
                id: NoteId(n as u32),
                text: format!("note {n}"),
                classification: Classification::CHANNEL_DEFAULT,
            };
            state.notes.push(note, Default::default(), id(n));
        }
    }

    fn same(a: &AgentChannelState, b: &AgentChannelState) -> bool {
        serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
    }

    #[tokio::test]
    async fn a_put_appends_what_changed_to_the_journal_and_a_get_replays_it() {
        let dir = tempdir().unwrap();
        let opened = store(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 3);
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&opened, agent, channel), None, "the first put is the snapshot");
        let snapshot =
            std_fs::metadata(opened.agent_channel_state_path(agent, channel)).unwrap().len();

        said(&mut state, 3, 5);
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        state.processing_state = ActorState::WaitingForUser;
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&opened, agent, channel), Some(2), "one line per put");
        assert_eq!(
            std_fs::metadata(opened.agent_channel_state_path(agent, channel)).unwrap().len(),
            snapshot,
            "the snapshot is untouched"
        );

        // Read from the files, by a store opened anew, as after a restart.
        let loaded = store(dir.path()).get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        assert_eq!(loaded.transcript.messages().len(), 4);
        assert_eq!(
            (loaded.last_message, loaded.heard.next()),
            (Some(id(4)), Position::new(5)),
            "where it stands in its channel's history (issue 440)"
        );
    }

    #[tokio::test]
    async fn the_journal_is_folded_into_the_snapshot_once_it_is_as_large() {
        let dir = tempdir().unwrap();
        let store = store(dir.path()).with_journal_floor(0);
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let path = store.agent_channel_state_path(agent, channel);
        let snapshot = std_fs::metadata(&path).unwrap().len();

        // Small puts go to the journal until it is as large as the snapshot.
        let mut puts = 0;
        while journal_lines(&store, agent, channel).is_some() || puts == 0 {
            state.processing_state = ActorState::Idle;
            store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
            puts += 1;
            assert!(puts < 1000, "the journal is never folded");
        }
        assert!(puts > 1, "the journal was used first");
        assert_eq!(
            std_fs::metadata(&path).unwrap().len(),
            snapshot,
            "the snapshot is the same state again"
        );

        said(&mut state, 2, 40);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(
            journal_lines(&store, agent, channel).is_some(),
            "a large put still goes to the journal"
        );
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(
            journal_lines(&store, agent, channel),
            None,
            "and the next one folds it, the journal being larger than the snapshot"
        );
        assert!(std_fs::metadata(&path).unwrap().len() > snapshot);
        assert!(same(&store.get_agent_channel_state(agent, channel).await.unwrap(), &state));
    }

    #[tokio::test]
    async fn a_torn_last_line_is_dropped_and_a_corrupt_line_before_is_not() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        said(&mut state, 2, 3);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let before = store.get_agent_channel_state(agent, channel).await.unwrap();

        let journal = journal_file(&store, agent, channel);
        let good = std_fs::read_to_string(&journal).unwrap();
        std_fs::write(&journal, format!("{good}[{{\"transcript\": {{\"entr")).unwrap();
        let loaded = store.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &before), "as of the last put that completed");

        std_fs::write(&journal, format!("not json\n{good}")).unwrap();
        let corrupt = store.get_agent_channel_state(agent, channel).await.unwrap_err();
        assert!(
            matches!(corrupt, StoreError::Backend(ref m) if m.contains("line 1") && !m.contains("not json")),
            "{corrupt}"
        );
    }

    #[tokio::test]
    async fn a_journal_that_survived_its_snapshot_replays_without_effect() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        said(&mut state, 2, 4);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let journal = journal_file(&store, agent, channel);
        let lines = std_fs::read_to_string(&journal).unwrap();

        // As if the crash came after the snapshot and before the journal was emptied.
        let folding = JsonFileStore::new(dir.path()).unwrap().with_journal_floor(0);
        folding.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(!journal.exists());
        std_fs::write(&journal, &lines).unwrap();

        let loaded = store.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        said(&mut state, 4, 5);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(
            same(&store.get_agent_channel_state(agent, channel).await.unwrap(), &state),
            "and what comes after it is applied"
        );
    }

    #[tokio::test]
    async fn items_behind_their_chain_and_entries_past_its_head_are_what_a_crash_leaves() {
        use caelumora_model::store::history::HistoryStore;
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let channel = ChannelId::new();
        let key = channel;
        let mut state = ChannelState::new(channel);
        state.history.extend(vec![make_message(id(1)), make_message(id(2)), make_message(id(3))]);
        store.put_channel_state(channel, &mut state).await.unwrap();
        let history = store.history_dir(key);
        let read = |file: &str| {
            serde_json::from_str::<serde_json::Value>(
                &std_fs::read_to_string(history.join(file)).unwrap(),
            )
            .unwrap()
        };
        let write = |file: &str, value: &serde_json::Value| {
            std_fs::write(history.join(file), value.to_string()).unwrap()
        };

        // As if the crash came after the chain and its head, before the items.
        let mut items = read("items-0.json");
        items.as_array_mut().unwrap().truncate(1);
        write("items-0.json", &items);
        write("items-head.json", &serde_json::json!({ "end": 1 }));
        let loaded = store.get_channel_state(channel).await.unwrap();
        assert_eq!(loaded.history.end().get(), 3);
        assert_eq!(
            store
                .page(key, &Reader::Channel, Position::START, None, 10, None)
                .await
                .unwrap()
                .items
                .len(),
            3,
            "the items are derived anew from the chain"
        );
        assert_eq!(read("items-head.json")["end"], serde_json::json!(3));

        // As if the crash came after the chain, before its head.
        let mut chain = read("chain-0.json");
        let mut past = chain[2].clone();
        past["seq"] = serde_json::json!(3);
        chain.as_array_mut().unwrap().push(past);
        write("chain-0.json", &chain);
        let mut loaded = store.get_channel_state(channel).await.unwrap();
        assert_eq!(loaded.history.end().get(), 3, "what lies past the head is not there");
        loaded.history.push(make_message(id(4)));
        store.put_channel_state(channel, &mut loaded).await.unwrap();
        assert_eq!(
            store.said(key, &Reader::Channel, id(4)).await.unwrap().map(|(at, _)| at.get()),
            Some(3),
            "and the next entry takes its place"
        );
        assert_eq!(
            store.get_channel_state(channel).await.unwrap().history.digest(),
            loaded.history.digest()
        );
    }

    #[tokio::test]
    async fn deleting_a_channels_state_removes_its_journal_too() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let channel = ChannelId::new();
        let mut state = ChannelState::new(channel);
        store.put_channel_state(channel, &mut state).await.unwrap();
        state.history.extend(vec![make_message(id(1))]);
        store.put_channel_state(channel, &mut state).await.unwrap();
        let journal = journal_path(&store.channel_state_path(channel));
        assert!(journal.exists());
        assert!(store.history_dir(channel).exists());
        store.delete_channel_state(channel).await.unwrap();
        assert!(!journal.exists() && !store.channel_state_path(channel).exists());
        assert!(!store.history_dir(channel).exists(), "and its history");
    }
}
