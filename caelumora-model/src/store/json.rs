use crate::agent::{AgentChannelState, AgentId, AgentState};
use crate::channel::{ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::confidentiality::ConfidentialityPolicy;
use crate::removal::{Deletable, Removal};
use crate::security::{ActionType, AccessPolicy};
use crate::store::access::{name_free, Access};
use crate::store::query::{Field, Filter, NoField, Page, Query};
use crate::store::{ComponentStore, EntityStore, EntityStoreFactory, EntityStores, Journaled, Policies, StateStore, StoreError, StoreResult, Stored};
use crate::type_map::TypeMap;
use crate::value::name::ScopedName;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::ffi::OsString;
use std::fmt::{self, Debug, Display};
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, MutexGuard};
use tracing::{debug, info, warn};

/// A journal is folded into its snapshot once it is at least as large as
/// the snapshot, and at least this large (issue 193): small states are not
/// written whole at every other put. `journal_floor` of the configuration.
pub const DEFAULT_JOURNAL_FLOOR: u64 = 64 * 1024;

/// One JSON file per document (`EntityStore`), and per state a snapshot
/// and a journal (`StateStore`, issue 193): the state as it was last written
/// whole in `<key>.json`, and what changed since as one line per put in
/// `<key>.jsonl`. Loading reads the snapshot and replays the journal; a
/// put appends a line, or writes a new snapshot and empties the journal
/// once the journal is at least as large as the snapshot and
/// `journal_floor`. What a crash leaves is read as it is: a torn last line
/// is dropped, and a journal that survived its snapshot replays onto it
/// without effect, since replaying is idempotent.
#[derive(Debug)]
pub struct JsonFileStore {
    root: PathBuf,
    journal_floor: u64,
    policies: Policies,
    /// The store of every kind asked for, by its type.
    kinds: RwLock<TypeMap>,
    components: JsonEntityStore<StoredComponents>,
}

impl JsonFileStore {
    /// A store of the files under `root` that asks `access_policy` about
    /// every access to an entity, and `confidentiality` whether the caller
    /// is cleared for what is reached by clearance.
    pub fn new(root: impl Into<PathBuf>, access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> StoreResult<Self> {
        let root = root.into();
        info!("Creating JSON file store at {:?}", root);
        let policies = (access_policy, confidentiality);
        let store = Self {
            journal_floor: DEFAULT_JOURNAL_FLOOR,
            components: JsonEntityStore::new(&root, &policies)?,
            policies,
            kinds: Default::default(),
            root,
        };
        store.ensure_layout()?;
        // The directories of the core's kinds, made now, so that a store
        // that cannot keep them is not opened at all.
        EntityStores::open(&store)?;
        Ok(store)
    }

    /// The stores of the core's entities, one per kind (issue 323).
    ///
    /// # Errors
    /// If the directory of one cannot be made.
    pub fn entity_stores(&self) -> StoreResult<EntityStores> {
        EntityStores::open(self)
    }

    /// With a journal folded into its snapshot once it is `bytes` large, if
    /// that is more than the snapshot.
    pub fn with_journal_floor(mut self, bytes: u64) -> Self {
        self.journal_floor = bytes;
        self
    }

    fn ensure_layout(&self) -> StoreResult<()> {
        for dir in [self.agent_states_dir(), self.agent_channel_states_dir(), self.channel_states_dir()] {
            fs::create_dir_all(dir).map_err(to_backend_error)?;
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
        self.agent_channel_state_dir(channel_id)
            .join(format!("{}.json", agent_id))
    }

    fn agent_channel_state_dir(&self, channel_id: ChannelId) -> PathBuf {
        self.agent_channel_states_dir().join(channel_id.to_string())
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
    if let Err(err) = tokio::fs::write(&tmp_path, payload).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    if let Err(err) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    Ok(())
}

async fn read_json_opt<T: DeserializeOwned>(path: &Path) -> StoreResult<Option<T>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(to_backend_error)?;
            Ok(Some(parsed))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// The documents in `dir` - every `*.json` file -, each with its key, the
/// file's name without `.json`, and its path; temporary files of writes in
/// progress, or left behind by a crash, are not among them. Only the
/// directory is read: a document is read when it is its turn
/// (`read_meeting`).
async fn documents_in_dir(dir: &Path) -> StoreResult<Vec<(String, PathBuf)>> {
    let mut documents = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await.map_err(to_backend_error)?;
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

        let key = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_string();
        documents.push((key, path));
    }
    Ok(documents)
}

/// The document at `path` as a `T`, if it meets `filter` - judged on it as
/// JSON, before it is read as its type. `None` as well if it is gone,
/// deleted since its directory was read, or cannot be read, which is
/// logged: one broken document does not break a list.
async fn read_meeting<T: DeserializeOwned, F: Field>(path: PathBuf, filter: Option<&Filter<F>>) -> StoreResult<Option<T>> {
    let document = match read_json_opt::<Value>(&path).await {
        Ok(Some(document)) => document,
        Ok(None) => return Ok(None),
        Err(err) => {
            warn!("Failed to read JSON file {:?}: {}", path, err);
            return Ok(None);
        }
    };
    if filter.is_some_and(|filter| !filter.matches(&document)) {
        return Ok(None);
    }
    match serde_json::from_value::<T>(document) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            warn!("Failed to read the document {:?}: {}", path, err);
            Ok(None)
        }
    }
}

async fn delete_if_exists(path: &Path) -> StoreResult<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

async fn delete_dir_if_exists(path: &Path) -> StoreResult<bool> {
    match tokio::fs::remove_dir_all(path).await {
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
    match tokio::fs::metadata(path).await {
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
    let text = match tokio::fs::read_to_string(&journal).await {
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
                warn!("The journal {} ends in a torn line, left by a crash; it is dropped: {}", journal.display(), err);
                break;
            }
            Err(err) => return Err(StoreError::Backend(format!("line {} of the journal of a stored state is corrupt: {err}", i + 1))),
        };
        for change in changes {
            state.apply(change).map_err(|e| StoreError::Backend(format!("line {} of the journal does not fit the stored state: {e}", i + 1)))?;
        }
    }
    Ok(Some(state))
}

/// Appends `changes` as one line to the journal at `journal`, which is
/// `len_before` bytes long. Should the write fail, the journal is cut back
/// to what it was, so that a torn line is never followed by a good one.
async fn append_changes<C: Serialize>(journal: &Path, len_before: u64, changes: &[C]) -> StoreResult<()> {
    let mut line = serde_json::to_vec(changes).map_err(to_backend_error)?;
    line.push(b'\n');
    let mut file = OpenOptions::new().append(true).create(true).open(journal).await.map_err(to_backend_error)?;
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

fn sanitize_file_component(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

use crate::security::SecurityContext;
use serde_json::Value;
use std::io::ErrorKind;
use tokio::fs::OpenOptions;
use uuid::Uuid;

/// Refuses `value` if another document in `dir` has its name (issue 250).
async fn name_free_in<T: Stored + DeserializeOwned>(dir: &Path, value: &T) -> StoreResult<()> {
    for (_, path) in documents_in_dir(dir).await? {
        if let Some(other) = read_meeting::<T, NoField>(path, None).await? {
            name_free(value, [&other])?;
        }
    }
    Ok(())
}

/// Opens a kind in the directory of its name.
impl EntityStoreFactory for JsonFileStore {
    type Error = StoreError;

    fn entity_store<T: Deletable>(&self) -> StoreResult<Arc<dyn EntityStore<T>>> {
        let mut kinds = self.kinds.write().unwrap_or_else(PoisonError::into_inner);
        kinds.get_or_try_insert(|| Ok(Arc::new(JsonEntityStore::<T>::new(&self.root, &self.policies)?) as Arc<dyn EntityStore<T>>))
    }
}

/// The entities of one kind as JSON files (issue 323), one per document in
/// the kind's directory, `entities/<kind>`, named by its id; checked as
/// every kind is (`store::access`). A write of a kind whose names are
/// unique holds the kind's lock from reading what is stored to writing,
/// and checks the others of the kind.
pub struct JsonEntityStore<T> {
    dir: PathBuf,
    access_policy: Arc<dyn AccessPolicy>,
    /// Who is cleared for what is reached by clearance (issue 322).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// Held by the writes that may take a name, so that no two take one:
    /// within this process, which is the only one that writes the files.
    names: Mutex<()>,
    kind: PhantomData<fn() -> T>,
}

impl<T: Stored> Debug for JsonEntityStore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonEntityStore").field("kind", &T::KIND).field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl<T: Stored> JsonEntityStore<T> {
    /// The store of the kind's documents under `root`, asking the
    /// `AccessPolicy` and the `ConfidentialityPolicy` of `policies`.
    ///
    /// # Errors
    /// If its directory cannot be made.
    fn new(root: &Path, policies: &Policies) -> StoreResult<Self> {
        let dir = root.join("entities").join(T::KIND.name());
        fs::create_dir_all(&dir).map_err(to_backend_error)?;
        Ok(Self { dir, access_policy: Arc::clone(&policies.0), confidentiality: Arc::clone(&policies.1), names: Mutex::new(()), kind: PhantomData })
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.access_policy.as_ref(), self.confidentiality.as_ref(), ctx)
    }

    /// Where the document of `id` is.
    fn path(&self, id: &T::Id) -> PathBuf {
        self.dir.join(format!("{}.json", sanitize_file_component(&id.to_string())))
    }

    /// The kind's lock for names, if `value` has one to take.
    async fn names_lock(&self, value: &T) -> Option<MutexGuard<'_, ()>> {
        match value.unique_name() {
            Some(_) => Some(self.names.lock().await),
            None => None,
        }
    }

    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        let id = value.id();
        let path = self.path(&id);
        let _names = self.names_lock(value).await;
        self.access(ctx).create(value).await?;
        if tokio::fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::Conflict(format!("{} {} already exists", T::KIND, id)));
        }
        name_free_in(&self.dir, value).await?;
        debug!("Writing {} {} to {}", T::KIND, id, path.display());
        write_json_atomic(&path, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let found = read_json_opt::<T>(&self.path(&id)).await?;
        self.access(ctx).read(found, id.to_string()).await
    }

    /// Reads the others of the kind only if `value` is named otherwise than
    /// it is stored: whatever else changes takes nobody's name.
    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        let id = value.id();
        let path = self.path(&id);
        let _names = self.names_lock(value).await;
        let stored = read_json_opt::<T>(&path).await?;
        self.access(ctx).change(stored.as_ref(), value, id.to_string()).await?;
        if stored.is_some_and(|stored| stored.unique_name() != value.unique_name()) {
            name_free_in(&self.dir, value).await?;
        }
        debug!("Writing {} {} to {}", T::KIND, id, path.display());
        write_json_atomic(&path, value).await
    }

    /// Removes the document for good: deleting a kind that is not marked.
    async fn remove(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let path = self.path(&id);
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        debug!("Deleting {} at {}", T::KIND, path.display());
        delete_if_exists(&path).await
    }

    /// The page `query` asks for, among what is deleted if `deleted`.
    async fn page(&self, ctx: &SecurityContext, query: &Query<T::Field>, deleted: bool) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let filter = query.filter.as_ref();
        access.page(documents_in_dir(&self.dir).await?, query, deleted, |path| read_meeting::<T, T::Field>(path, filter)).await
    }
}

#[async_trait]
impl<T: Deletable> EntityStore<T> for JsonEntityStore<T> {
    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        JsonEntityStore::create(self, ctx, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        JsonEntityStore::get(self, ctx, id).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        JsonEntityStore::update(self, ctx, value).await
    }

    /// Marks the document deleted, by the caller and now (issue 164),
    /// rewriting it where it is.
    async fn delete(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let path = self.path(&id);
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        debug!("Marking {} at {} deleted", T::KIND, path.display());
        write_json_atomic(&path, &marked).await?;
        Ok(true)
    }

    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, false).await
    }

    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let path = self.path(&id);
        // Taken before reading: what is restored may take a name.
        let _names = self.names.lock().await;
        let stored = read_json_opt::<T>(&path).await?;
        let mut restored = self.access(ctx).restore(stored, id.to_string()).await?;
        restored.set_deleted(None);
        name_free_in(&self.dir, &restored).await?;
        debug!("Restoring {} at {}", T::KIND, path.display());
        write_json_atomic(&path, &restored).await?;
        Ok(restored)
    }

    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let path = self.path(&id);
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).purge(stored.as_ref()).await? {
            return Ok(false);
        }
        // Read once more right before: restored while the purge was asked
        // about, it stays. A file cannot be removed on that condition
        // atomically; whoever restores and purges at once serialises them
        // (`SessionManager`).
        if read_json_opt::<T>(&path).await?.is_none_or(|value| value.deleted().is_none()) {
            return Ok(false);
        }
        debug!("Purging {} at {}", T::KIND, path.display());
        delete_if_exists(&path).await
    }

    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let found = read_json_opt::<T>(&self.path(&id)).await?;
        self.access(ctx).read_deleted(found, id.to_string()).await
    }

    async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, true).await
    }

    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let all = self.page(ctx, &Query::all(), false).await?.items;
        all.into_iter().find(|value| value.unique_name().as_ref() == Some(name))
            .ok_or_else(|| StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() })
    }
}

/// Preconfigured components, kept like the entities (issue 164).
#[async_trait]
impl ComponentStore for JsonFileStore {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.create(ctx, value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.components.get(ctx, id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.update(ctx, value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.components.remove(ctx, id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.components.page(ctx, query, false).await
    }
}

/// The state operations: a snapshot and a journal per state (issue 193).
impl JsonFileStore {
    /// Stores `state` at `path`, and marks it stored: as a line of its
    /// journal, or - if there is no snapshot yet, or the journal is as large
    /// as the snapshot and `journal_floor` - as a new snapshot, after which
    /// the journal is emptied. The sizes are asked of the file system every
    /// time; nothing is kept between puts.
    async fn store_state<T: Journaled>(&self, path: &Path, state: &mut T) -> StoreResult<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(to_backend_error)?;
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
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        let path = self.agent_channel_state_path(agent_id, channel_id);
        debug!("Reading agent channel state from {}", path.display());
        Ok(load_state(&path).await?.unwrap_or_default())
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()> {
        self.store_state(&self.agent_channel_state_path(agent_id, channel_id), state).await
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
        load_state(&path).await?.ok_or(StoreError::NotFound {
            entity: "channel_state",
            id: channel_id.to_string(),
        })
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &mut ChannelState) -> StoreResult<()> {
        self.store_state(&self.channel_state_path(channel_id), state).await
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        debug!("Deleting the state of channel {}", channel_id);
        let path = self.channel_state_path(channel_id);
        delete_if_exists(&path).await?;
        delete_if_exists(&journal_path(&path)).await?;
        delete_dir_if_exists(&self.agent_channel_state_dir(channel_id)).await?;
        Ok(())
    }

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        delete_if_exists(&path).await?;
        delete_if_exists(&journal_path(&path)).await?;
        let mut channels = match tokio::fs::read_dir(self.agent_channel_states_dir()).await {
            Ok(channels) => channels,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(to_backend_error(err)),
        };
        while let Some(channel) = channels.next_entry().await.map_err(to_backend_error)? {
            let path = channel.path().join(format!("{agent_id}.json"));
            delete_if_exists(&path).await?;
            delete_if_exists(&journal_path(&path)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::session::{Session, SessionId};
    use super::*;
    use chrono::Utc;
    use crate::security::SecurityAttributes;
    use crate::store::suite;
    use tempfile::tempdir;
    use uuid::Uuid;
    use crate::confidentiality::{ConfidentialityRules, Classification};
    use crate::security::{AllowAllPolicy, OwnershipPolicy};

    fn store(dir: &Path) -> JsonFileStore {
        JsonFileStore::new(dir, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap()
    }

    /// The store of the kind `T` in the files of `store`, with what only it
    /// has: where its documents lie.
    fn of_kind<T: Stored>(store: &JsonFileStore) -> JsonEntityStore<T> {
        JsonEntityStore::new(&store.root, &store.policies).unwrap()
    }

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        let dir = tempdir().unwrap();
        suite::entity_store_behaves(&Arc::new(store(dir.path())).entity_stores().unwrap()).await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        suite::queries_behave(&store.entity_stores().unwrap()).await;
        suite::queries_reach_parts(store.entity_store::<suite::Folder>().unwrap().as_ref()).await;
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        let dir = tempdir().unwrap();
        suite::state_store_behaves(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn behaves_as_a_component_store() {
        let dir = tempdir().unwrap();
        suite::component_store_behaves(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        let dir = tempdir().unwrap();
        suite::deleting_marks(&Arc::new(store(dir.path())).entity_stores().unwrap()).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        let dir = tempdir().unwrap();
        suite::names_are_unique(&Arc::new(store(dir.path())).entity_stores().unwrap()).await;
        let dir = tempdir().unwrap();
        let owners = JsonFileStore::new(dir.path(), Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap();
        suite::names_are_unique_beyond_what_is_read(&Arc::new(owners).entity_stores().unwrap()).await;
    }

    #[tokio::test]
    async fn a_document_stored_before_marks_existed_loads_and_one_not_deleted_is_written_without() {
        let dir = tempdir().unwrap();
        let sessions = of_kind::<Session>(&store(dir.path()));
        let system = SecurityContext::system();
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionId(Uuid::new_v4()), name: Name::new("old").unwrap(), participants: Vec::new(), channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), created_at: Utc::now(), updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(), deleted: None,
        };
        sessions.create(&system, &session).await.unwrap();
        let text = tokio::fs::read_to_string(sessions.path(&session.id)).await.unwrap();
        assert!(!text.contains("deleted"), "{text}");
        assert!(sessions.get(&system, session.id).await.unwrap().deleted.is_none());
    }

    #[tokio::test]
    async fn concurrent_writes_of_one_file_all_succeed_and_leave_no_temporary_files() {
        // Entities may be written at the same time - by two requests that
        // change one (issue 62); a state has one writer (issue 215).
        use crate::agent::{Personality, PersonalityId};
        let dir = tempdir().unwrap();
        let personalities = Arc::new(of_kind::<Personality>(&store(dir.path())));
        let system = SecurityContext::system();
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, description: None, introduction: None,
            components: Default::default(), security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        personalities.create(&system, &personality).await.unwrap();

        let mut writes = JoinSet::new();
        for i in 0..16 {
            let (personalities, system) = (Arc::clone(&personalities), system.clone());
            let changed = Personality { description: Some(Description::new(format!("version {i}")).unwrap()), ..personality.clone() };
            writes.spawn(async move { personalities.update(&system, &changed).await });
        }
        while let Some(result) = writes.join_next().await {
            result.unwrap().expect("no writer loses its temporary file to another");
        }

        let path = personalities.path(&personality.id);
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [path.file_name().unwrap().to_string_lossy().into_owned()], "only the file itself");
        let stored = personalities.get(&system, personality.id).await.expect("readable");
        assert!(stored.description.as_ref().map(|d| d.as_str()).unwrap_or_default().starts_with("version "), "one of the writes, whole");
    }

    #[tokio::test]
    async fn of_concurrent_writes_under_one_name_one_takes_it() {
        // Checked and written under the kind's lock (issue 250): nothing
        // writes between another's check and its write.
        use crate::actor::ActorId;
        use crate::agent::{Agent, Personality, PersonalityId};
        let dir = tempdir().unwrap();
        let agents = Arc::new(of_kind::<Agent>(&store(dir.path())));
        let system = SecurityContext::system();
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, description: None, introduction: None,
            components: Default::default(), security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };

        let mut writes = JoinSet::new();
        for _ in 0..16 {
            let (agents, system) = (Arc::clone(&agents), system.clone());
            let agent = Agent::new(ScopedName::new("Helper", Default::default()).unwrap(), personality.id, ActorId::None);
            writes.spawn(async move { agents.create(&system, &agent).await });
        }
        let mut made = 0;
        while let Some(result) = writes.join_next().await {
            match result.unwrap() {
                Ok(()) => made += 1,
                Err(e) => assert!(matches!(e, StoreError::NameTaken { entity: "agent" }), "refused for its name: {e:?}"),
            }
        }
        assert_eq!(made, 1);
        assert_eq!(agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deleting_a_session_marks_its_file_and_purging_removes_it_leaving_its_state_to_the_state_store() {
        use crate::channel::{Channel, ChannelId};

        let dir = tempdir().unwrap();
        let access_policy = Arc::new(AllowAllPolicy::new());
        let store = JsonFileStore::new(dir.path(), access_policy, Arc::new(ConfidentialityRules::default())).unwrap();
        let sessions = of_kind::<Session>(&store);
        let ctx = SecurityContext::default();

        let session_id = SessionId(Uuid::new_v4());
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            id: ChannelId(Uuid::new_v4()),
            session_id,
            name: Name::new("general").unwrap(),
            components: moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            former: Vec::new(),
            participants: Vec::new(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        let channel_id = channel.id;
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: session_id,
            name: Name::new("to-delete").unwrap(),
            participants: Vec::new(),
            channels: vec![channel.id],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        sessions.create(&ctx, &session).await.unwrap();

        // Write out channel state so we can assert it gets removed.
        let agent_id = AgentId(Uuid::new_v4());
        store
            .put_agent_channel_state(agent_id, channel_id, &mut AgentChannelState::default())
            .await
            .unwrap();

        assert!(sessions.path(&session_id).exists());
        assert!(store.agent_channel_state_dir(channel_id).exists());

        assert!(sessions.delete(&ctx, session_id).await.unwrap());
        assert!(sessions.path(&session_id).exists(), "deleting marks the file");
        assert!(store.agent_channel_state_dir(channel_id).exists(), "the state is the state store's");
        assert!(sessions.purge(&ctx, session_id).await.unwrap());
        assert!(!sessions.path(&session_id).exists(), "purging removes it");

        store.put_channel_state(channel_id, &mut ChannelState::new(channel_id)).await.unwrap();
        store.delete_channel_state(channel_id).await.unwrap();
        assert!(!store.channel_state_path(channel_id).exists());
        assert!(!store.agent_channel_state_dir(channel_id).exists());
        store.delete_channel_state(channel_id).await.expect("nothing to delete is no error");

        // Deleting or purging again reports that nothing was there.
        assert!(!sessions.delete(&ctx, session_id).await.unwrap());
        assert!(!sessions.purge(&ctx, session_id).await.unwrap());
        let err = sessions.get(&ctx, session_id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "session", .. }));
    }

    #[tokio::test]
    async fn list_sessions_skips_corrupted_files() {
        let dir = tempdir().unwrap();
        let access_policy = Arc::new(AllowAllPolicy::new());
        let store = JsonFileStore::new(dir.path(), access_policy, Arc::new(ConfidentialityRules::default())).unwrap();
        let sessions = of_kind::<Session>(&store);
        let ctx = SecurityContext::default();

        let s1 = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: Name::new("session-1").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        sessions.create(&ctx, &s1).await.unwrap();

        // Create a corrupted JSON file
        let corrupted_path = sessions.dir.join("corrupted.json");
        fs::write(corrupted_path, "invalid json").unwrap();

        // This should NOT fail, it should return s1 and log a warning
        let sessions = sessions.list(&ctx, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, s1.id);
    }

    use crate::message::tests::{id, make_message};
    use crate::actor::ActorState;
    use crate::channel::moderation_components;
    use crate::value::name::Name;
    use crate::value::text::Description;
    use tokio::task::JoinSet;

    /// The lines of the journal of the agent channel state of `agent` in
    /// `channel`, `None` if there is no journal.
    fn journal_lines(store: &JsonFileStore, agent: AgentId, channel: ChannelId) -> Option<usize> {
        std::fs::read_to_string(journal_path(&store.agent_channel_state_path(agent, channel))).ok().map(|text| text.lines().count())
    }

    fn journal_file(store: &JsonFileStore, agent: AgentId, channel: ChannelId) -> PathBuf {
        journal_path(&store.agent_channel_state_path(agent, channel))
    }

    /// A state with messages `from..to` in its transcript and history.
    fn said(state: &mut AgentChannelState, from: u128, to: u128) {
        for n in from..to {
            state.transcript.push(make_message(id(n)));
            state.history.extend(vec![make_message(id(n))]);
        }
    }

    fn same(a: &AgentChannelState, b: &AgentChannelState) -> bool {
        serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
    }

    #[tokio::test]
    async fn a_put_appends_what_changed_to_the_journal_and_a_get_replays_it() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 3);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&store, agent, channel), None, "the first put is the snapshot");
        let snapshot = std::fs::metadata(store.agent_channel_state_path(agent, channel)).unwrap().len();

        said(&mut state, 3, 5);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        state.processing_state = ActorState::WaitingForUser;
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&store, agent, channel), Some(2), "one line per put");
        assert_eq!(std::fs::metadata(store.agent_channel_state_path(agent, channel)).unwrap().len(), snapshot, "the snapshot is untouched");

        let loaded = store.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        assert_eq!(loaded.transcript.messages().len(), 4);
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
        let snapshot = std::fs::metadata(&path).unwrap().len();

        // Small puts go to the journal until it is as large as the snapshot.
        let mut puts = 0;
        while journal_lines(&store, agent, channel).is_some() || puts == 0 {
            state.processing_state = ActorState::Idle;
            store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
            puts += 1;
            assert!(puts < 1000, "the journal is never folded");
        }
        assert!(puts > 1, "the journal was used first");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), snapshot, "the snapshot is the same state again");

        said(&mut state, 2, 40);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(journal_lines(&store, agent, channel).is_some(), "a large put still goes to the journal");
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&store, agent, channel), None, "and the next one folds it, the journal being larger than the snapshot");
        assert!(std::fs::metadata(&path).unwrap().len() > snapshot);
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
        let good = std::fs::read_to_string(&journal).unwrap();
        std::fs::write(&journal, format!("{good}[{{\"transcript\": {{\"entr")).unwrap();
        let loaded = store.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &before), "as of the last put that completed");

        std::fs::write(&journal, format!("not json\n{good}")).unwrap();
        let corrupt = store.get_agent_channel_state(agent, channel).await.unwrap_err();
        assert!(matches!(corrupt, StoreError::Backend(ref m) if m.contains("line 1") && !m.contains("not json")), "{corrupt}");
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
        let lines = std::fs::read_to_string(&journal).unwrap();

        // As if the crash came after the snapshot and before the journal was emptied.
        let folding = JsonFileStore::new(dir.path(), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap().with_journal_floor(0);
        folding.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(!journal.exists());
        std::fs::write(&journal, &lines).unwrap();

        let loaded = store.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        said(&mut state, 4, 5);
        store.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(same(&store.get_agent_channel_state(agent, channel).await.unwrap(), &state), "and what comes after it is applied");
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
        store.delete_channel_state(channel).await.unwrap();
        assert!(!journal.exists() && !store.channel_state_path(channel).exists());
    }
}
