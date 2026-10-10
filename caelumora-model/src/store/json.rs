use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::removal::{Deletable, Removal, Stored};
use crate::scope::Scoped;
use crate::security::SecurityManager;
use crate::value::name::ScopedName;
use crate::store::access::{name_free, Access, Actions, AGENTS, ARTIFACTS, CHANNELS, COMPONENTS, PERSONALITIES, SESSIONS, SESSION_TEMPLATES, SKILLS, USERS, USER_PROFILES, WORKSPACES};
use crate::session::{Session, SessionId};
use crate::store::{ComponentStore, EntityKind, EntityStore, Journaled, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId, UserProfile};
use crate::workspace::{Workspace, WorkspaceId};
use crate::skill::{Skill, SkillId};
use crate::artifact::{Artifact, ArtifactId};
use async_trait::async_trait;
use tracing::{debug, info, warn};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, MutexGuard};
use crate::store::query::{Field, Filter, NoField, Page, Query};
use crate::artifact::ArtifactField;
use crate::channel::ChannelField;
use crate::scope::ScopeField;
use crate::session::SessionField;

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
    security_manager: Arc<dyn SecurityManager>,
    journal_floor: u64,
    /// One per kind whose names are unique (issue 250), held by its writes:
    /// within this process, which is the only one that writes the files.
    name_locks: HashMap<EntityKind, Mutex<()>>,
}

impl JsonFileStore {
    pub fn new(root: impl Into<PathBuf>, security_manager: Arc<dyn SecurityManager>) -> StoreResult<Self> {
        let root = root.into();
        info!("Creating JSON file store at {:?}", root);
        let store = Self {
            root,
            security_manager,
            journal_floor: DEFAULT_JOURNAL_FLOOR,
            name_locks: [EntityKind::Personality, EntityKind::SessionTemplate, EntityKind::Agent, EntityKind::Channel, EntityKind::User, EntityKind::Workspace, EntityKind::Skill, EntityKind::Artifact]
                .into_iter().map(|kind| (kind, Mutex::new(()))).collect(),
        };
        store.ensure_layout()?;
        Ok(store)
    }

    /// With a journal folded into its snapshot once it is `bytes` large, if
    /// that is more than the snapshot.
    pub fn with_journal_floor(mut self, bytes: u64) -> Self {
        self.journal_floor = bytes;
        self
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.security_manager.as_ref(), ctx)
    }

    fn ensure_layout(&self) -> StoreResult<()> {
        let dirs = [
            self.personality_dir(),
            self.session_templates_dir(),
            self.agents_dir(),
            self.sessions_dir(),
            self.channels_dir(),
            self.users_dir(),
            self.user_profiles_dir(),
            self.workspaces_dir(),
            self.skills_dir(),
            self.artifacts_dir(),
            self.agent_states_dir(),
            self.agent_channel_states_dir(),
            self.channel_states_dir(),
            self.components_dir(),
        ];

        for dir in dirs {
            fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(())
    }

    fn personality_dir(&self) -> PathBuf {
        self.root.join("templates").join("agents")
    }

    fn session_templates_dir(&self) -> PathBuf {
        self.root.join("templates").join("sessions")
    }

    fn agents_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agents")
    }

    fn users_dir(&self) -> PathBuf {
        self.root.join("runtime").join("users")
    }

    fn user_profiles_dir(&self) -> PathBuf {
        self.root.join("runtime").join("user_profiles")
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.root.join("runtime").join("workspaces")
    }

    fn skills_dir(&self) -> PathBuf {
        self.root.join("runtime").join("skills")
    }

    fn artifacts_dir(&self) -> PathBuf {
        self.root.join("runtime").join("artifacts")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("runtime").join("sessions")
    }

    fn channels_dir(&self) -> PathBuf {
        self.root.join("runtime").join("channels")
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

    fn components_dir(&self) -> PathBuf {
        self.root.join("templates").join("components")
    }

    fn personality_path(&self, id: PersonalityId) -> PathBuf {
        self.personality_dir().join(format!("{id}.json"))
    }

    fn session_template_path(&self, id: SessionTemplateId) -> PathBuf {
        self.session_templates_dir().join(format!("{id}.json"))
    }

    fn agent_path(&self, id: AgentId) -> PathBuf {
        self.agents_dir().join(format!("{id}.json"))
    }

    fn session_path(&self, id: SessionId) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn channel_path(&self, id: ChannelId) -> PathBuf {
        self.channels_dir().join(format!("{id}.json"))
    }

    fn user_path(&self, id: UserId) -> PathBuf {
        self.users_dir().join(format!("{id}.json"))
    }

    fn user_profile_path(&self, id: UserId) -> PathBuf {
        self.user_profiles_dir().join(format!("{id}.json"))
    }

    fn workspace_path(&self, id: WorkspaceId) -> PathBuf {
        self.workspaces_dir().join(format!("{id}.json"))
    }

    fn skill_path(&self, id: SkillId) -> PathBuf {
        self.skills_dir().join(format!("{id}.json"))
    }

    fn artifact_path(&self, id: ArtifactId) -> PathBuf {
        self.artifacts_dir().join(format!("{id}.json"))
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

    fn component_path(&self, id: &str) -> PathBuf {
        self.components_dir()
            .join(format!("{}.json", sanitize_file_component(id)))
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
    tmp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

fn require_entity<T>(value: Option<T>, entity: &'static str, id: String) -> StoreResult<T> {
    value.ok_or(StoreError::NotFound { entity, id })
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
    let document = match read_json_opt::<serde_json::Value>(&path).await {
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

async fn delete_dir_if_exists(path: &Path) -> StoreResult<bool> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Some(state)),
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
    let mut file = tokio::fs::OpenOptions::new().append(true).create(true).open(journal).await.map_err(to_backend_error)?;
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

/// The directory of the document at `path`: where the others of its kind
/// are.
fn dir_of(path: &Path) -> StoreResult<PathBuf> {
    path.parent().map(Path::to_path_buf).ok_or_else(|| StoreError::Backend(format!("invalid path: {}", path.display())))
}

/// Refuses `value` if another document in `dir` has its name (issue 250).
async fn name_free_in<T: Stored + Scoped + DeserializeOwned>(dir: &Path, value: &T, kind: &Actions) -> StoreResult<()> {
    for (_, path) in documents_in_dir(dir).await? {
        if let Some(other) = read_meeting::<T, NoField>(path, None).await? {
            name_free(value, [&other], kind)?;
        }
    }
    Ok(())
}

/// The operations of every kind (issues 93, 164), each document a file at
/// `path` and checked with its kind's `Actions`. Those that write come as
/// three: the plain one, the one for a kind whose names are unique
/// (`…_named_file`, issue 250), which holds the kind's lock from reading
/// what is stored to writing and checks the others of the kind, and what
/// both are made of (`…_file_with`), which asks `others` once the access
/// is granted.
impl JsonFileStore {
    /// Held by a write of a kind whose names are unique, so that no two
    /// writes take one name.
    async fn names_lock(&self, kind: &Actions) -> StoreResult<MutexGuard<'_, ()>> {
        let lock = self.name_locks.get(&kind.entity)
            .ok_or_else(|| StoreError::Backend(format!("the names of a {} are not kept", kind.entity)))?;
        Ok(lock.lock().await)
    }

    async fn create_file<T: Stored + Serialize + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        self.create_file_with(ctx, kind, path, id, value, async |_| Ok(())).await
    }

    async fn create_named_file<T: Stored + Scoped + Serialize + DeserializeOwned + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        let dir = dir_of(&path)?;
        let _names = self.names_lock(kind).await?;
        self.create_file_with(ctx, kind, path, id, value, async |value| name_free_in(&dir, value, kind).await).await
    }

    async fn create_file_with<T: Stored + Serialize + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T, others: impl AsyncFnOnce(&T) -> StoreResult<()>) -> StoreResult<()> {
        self.access(ctx).create(value, kind).await?;
        if tokio::fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::Conflict(format!("{} {} already exists", kind.entity, id)));
        }
        others(value).await?;
        debug!("Writing {} {} to {}", kind.entity, id, path.display());
        write_json_atomic(&path, value).await
    }

    async fn get_file<T: Stored + DeserializeOwned + Send>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String) -> StoreResult<T> {
        let found = read_json_opt::<T>(&path).await?;
        self.access(ctx).read(found, kind, id).await
    }

    async fn update_file<T: Stored + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        self.update_file_with(ctx, kind, path, id, value, async |_, _| Ok(())).await
    }

    /// Reads the others of the kind only if `value` is named otherwise than
    /// it is stored: whatever else changes takes nobody's name.
    async fn update_named_file<T: Stored + Scoped + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        let dir = dir_of(&path)?;
        let _names = self.names_lock(kind).await?;
        self.update_file_with(ctx, kind, path, id, value, async |stored, value| match stored.scoped_name() == value.scoped_name() {
            true => Ok(()),
            false => name_free_in(&dir, value, kind).await,
        }).await
    }

    /// `others` is asked with the entity as stored and as it is to be.
    async fn update_file_with<T: Stored + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T, others: impl AsyncFnOnce(&T, &T) -> StoreResult<()>) -> StoreResult<()> {
        let stored = read_json_opt::<T>(&path).await?;
        self.access(ctx).change(stored.as_ref(), value, kind, id.clone()).await?;
        if let Some(stored) = &stored {
            others(stored, value).await?;
        }
        debug!("Writing {} {} to {}", kind.entity, id, path.display());
        write_json_atomic(&path, value).await
    }

    /// Marks the document deleted, by the caller and now (issue 164),
    /// rewriting it where it is.
    async fn delete_file<T: Deletable + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf) -> StoreResult<bool> {
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).delete(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        debug!("Marking {} at {} deleted", kind.entity, path.display());
        write_json_atomic(&path, &marked).await?;
        Ok(true)
    }

    /// Removes the document for good: deleting a kind that is not marked.
    async fn remove_file<T: Stored + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf) -> StoreResult<bool> {
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).delete(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        debug!("Deleting {} at {}", kind.entity, path.display());
        delete_if_exists(&path).await
    }

    async fn restore_file<T: Deletable + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String) -> StoreResult<()> {
        self.restore_file_with::<T>(ctx, kind, path, id, async |_| Ok(())).await
    }

    async fn restore_named_file<T: Deletable + Scoped + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String) -> StoreResult<()> {
        let dir = dir_of(&path)?;
        let _names = self.names_lock(kind).await?;
        self.restore_file_with::<T>(ctx, kind, path, id, async |value| name_free_in(&dir, value, kind).await).await
    }

    async fn restore_file_with<T: Deletable + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, others: impl AsyncFnOnce(&T) -> StoreResult<()>) -> StoreResult<()> {
        let stored = read_json_opt::<T>(&path).await?;
        self.access(ctx).restore(stored.as_ref(), kind, id).await?;
        let Some(mut restored) = stored else { return Ok(()) };
        restored.set_deleted(None);
        others(&restored).await?;
        debug!("Restoring {} at {}", kind.entity, path.display());
        write_json_atomic(&path, &restored).await
    }

    async fn purge_file<T: Deletable + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf) -> StoreResult<bool> {
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).purge(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        // Read once more right before: restored while the purge was asked
        // about, it stays. A file cannot be removed on that condition
        // atomically; whoever restores and purges at once serialises them
        // (`SessionManager`).
        if read_json_opt::<T>(&path).await?.is_none_or(|value| value.deleted().is_none()) {
            return Ok(false);
        }
        debug!("Purging {} at {}", kind.entity, path.display());
        delete_if_exists(&path).await
    }

    async fn get_deleted_file<T: Deletable + DeserializeOwned + Send>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String) -> StoreResult<T> {
        let found = read_json_opt::<T>(&path).await?;
        self.access(ctx).read_deleted(found, kind, id).await
    }

    async fn list_deleted_dir<T: Deletable + DeserializeOwned + Send, F: Field>(&self, ctx: &SecurityContext, kind: &Actions, dir: PathBuf, query: &Query<F>) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let filter = query.filter.as_ref();
        access.page(documents_in_dir(&dir).await?, query, kind, true, |path| read_meeting::<T, F>(path, filter)).await
    }

    async fn list_dir<T: Stored + DeserializeOwned + Send, F: Field>(&self, ctx: &SecurityContext, kind: &Actions, dir: PathBuf, query: &Query<F>) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let filter = query.filter.as_ref();
        access.page(documents_in_dir(&dir).await?, query, kind, false, |path| read_meeting::<T, F>(path, filter)).await
    }

    /// The one of `name`, in exactly its scope, among those listed.
    async fn find_dir<T: Stored + Scoped + DeserializeOwned + Send>(&self, ctx: &SecurityContext, kind: &Actions, dir: PathBuf, name: &ScopedName) -> StoreResult<T> {
        let all: Vec<T> = self.list_dir(ctx, kind, dir, &Query::<NoField>::all()).await?.items;
        require_entity(all.into_iter().find(|d| d.scoped_name() == *name), kind.entity.name(), name.to_string())
    }
}

#[async_trait]
impl EntityStore for JsonFileStore {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.create_named_file(ctx, &PERSONALITIES, self.personality_path(value.id), value.id.to_string(), value).await
    }
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_file(ctx, &PERSONALITIES, self.personality_path(id), id.to_string()).await
    }
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.update_named_file(ctx, &PERSONALITIES, self.personality_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.delete_file::<Personality>(ctx, &PERSONALITIES, self.personality_path(id)).await
    }
    async fn list_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>> {
        self.list_dir(ctx, &PERSONALITIES, self.personality_dir(), query).await
    }
    async fn restore_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<()> {
        self.restore_named_file::<Personality>(ctx, &PERSONALITIES, self.personality_path(id), id.to_string()).await
    }
    async fn purge_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.purge_file::<Personality>(ctx, &PERSONALITIES, self.personality_path(id)).await
    }
    async fn get_deleted_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_deleted_file(ctx, &PERSONALITIES, self.personality_path(id), id.to_string()).await
    }
    async fn list_deleted_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>> {
        self.list_deleted_dir(ctx, &PERSONALITIES, self.personality_dir(), query).await
    }
    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Personality> {
        self.find_dir(ctx, &PERSONALITIES, self.personality_dir(), name).await
    }

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.create_named_file(ctx, &SESSION_TEMPLATES, self.session_template_path(value.id), value.id.to_string(), value).await
    }
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_file(ctx, &SESSION_TEMPLATES, self.session_template_path(id), id.to_string()).await
    }
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.update_named_file(ctx, &SESSION_TEMPLATES, self.session_template_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.delete_file::<SessionTemplate>(ctx, &SESSION_TEMPLATES, self.session_template_path(id)).await
    }
    async fn list_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>> {
        self.list_dir(ctx, &SESSION_TEMPLATES, self.session_templates_dir(), query).await
    }
    async fn restore_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<()> {
        self.restore_named_file::<SessionTemplate>(ctx, &SESSION_TEMPLATES, self.session_template_path(id), id.to_string()).await
    }
    async fn purge_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.purge_file::<SessionTemplate>(ctx, &SESSION_TEMPLATES, self.session_template_path(id)).await
    }
    async fn get_deleted_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_deleted_file(ctx, &SESSION_TEMPLATES, self.session_template_path(id), id.to_string()).await
    }
    async fn list_deleted_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>> {
        self.list_deleted_dir(ctx, &SESSION_TEMPLATES, self.session_templates_dir(), query).await
    }
    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<SessionTemplate> {
        self.find_dir(ctx, &SESSION_TEMPLATES, self.session_templates_dir(), name).await
    }

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.create_named_file(ctx, &AGENTS, self.agent_path(value.id), value.id.to_string(), value).await
    }
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_file(ctx, &AGENTS, self.agent_path(id), id.to_string()).await
    }
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.update_named_file(ctx, &AGENTS, self.agent_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.delete_file::<Agent>(ctx, &AGENTS, self.agent_path(id)).await
    }
    async fn list_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.list_dir(ctx, &AGENTS, self.agents_dir(), query).await
    }
    async fn restore_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<()> {
        self.restore_named_file::<Agent>(ctx, &AGENTS, self.agent_path(id), id.to_string()).await
    }
    async fn purge_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.purge_file::<Agent>(ctx, &AGENTS, self.agent_path(id)).await
    }
    async fn get_deleted_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_deleted_file(ctx, &AGENTS, self.agent_path(id), id.to_string()).await
    }
    async fn list_deleted_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.list_deleted_dir(ctx, &AGENTS, self.agents_dir(), query).await
    }
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Agent> {
        self.find_dir(ctx, &AGENTS, self.agents_dir(), name).await
    }

    // Sessions - their state is the `StateStore`'s, which may be another
    // store: whoever deletes a session deletes it there.
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.create_file(ctx, &SESSIONS, self.session_path(value.id), value.id.to_string(), value).await
    }
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.get_file(ctx, &SESSIONS, self.session_path(id), id.to_string()).await
    }
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.update_file(ctx, &SESSIONS, self.session_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.delete_file::<Session>(ctx, &SESSIONS, self.session_path(id)).await
    }
    async fn list_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.list_dir(ctx, &SESSIONS, self.sessions_dir(), query).await
    }
    async fn restore_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<()> {
        self.restore_file::<Session>(ctx, &SESSIONS, self.session_path(id), id.to_string()).await
    }
    async fn purge_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.purge_file::<Session>(ctx, &SESSIONS, self.session_path(id)).await
    }
    async fn get_deleted_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.get_deleted_file(ctx, &SESSIONS, self.session_path(id), id.to_string()).await
    }
    async fn list_deleted_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.list_deleted_dir(ctx, &SESSIONS, self.sessions_dir(), query).await
    }

    // Channels (issue 70) - their state likewise.
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.create_named_file(ctx, &CHANNELS, self.channel_path(value.id), value.id.to_string(), value).await
    }
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_file(ctx, &CHANNELS, self.channel_path(id), id.to_string()).await
    }
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.update_named_file(ctx, &CHANNELS, self.channel_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.delete_file::<Channel>(ctx, &CHANNELS, self.channel_path(id)).await
    }
    async fn list_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.list_dir(ctx, &CHANNELS, self.channels_dir(), query).await
    }
    async fn restore_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<()> {
        self.restore_named_file::<Channel>(ctx, &CHANNELS, self.channel_path(id), id.to_string()).await
    }
    async fn purge_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.purge_file::<Channel>(ctx, &CHANNELS, self.channel_path(id)).await
    }
    async fn get_deleted_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_deleted_file(ctx, &CHANNELS, self.channel_path(id), id.to_string()).await
    }
    async fn list_deleted_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.list_deleted_dir(ctx, &CHANNELS, self.channels_dir(), query).await
    }

    // Users - found by name as well, since a username is an identity
    // (issue 93).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.create_named_file(ctx, &USERS, self.user_path(value.id), value.id.to_string(), value).await
    }
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.get_file(ctx, &USERS, self.user_path(id), id.to_string()).await
    }
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.update_named_file(ctx, &USERS, self.user_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.delete_file::<User>(ctx, &USERS, self.user_path(id)).await
    }
    async fn list_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.list_dir(ctx, &USERS, self.users_dir(), query).await
    }
    async fn restore_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()> {
        self.restore_named_file::<User>(ctx, &USERS, self.user_path(id), id.to_string()).await
    }
    async fn purge_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.purge_file::<User>(ctx, &USERS, self.user_path(id)).await
    }
    async fn get_deleted_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.get_deleted_file(ctx, &USERS, self.user_path(id), id.to_string()).await
    }
    async fn list_deleted_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.list_deleted_dir(ctx, &USERS, self.users_dir(), query).await
    }
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let users: Vec<User> = self.list_dir(ctx, &USERS, self.users_dir(), &Query::<NoField>::all()).await?.items;
        require_entity(users.into_iter().find(|u| u.username == name), "user", name.to_string())
    }

    // User profiles (issue 176)
    async fn create_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()> {
        self.create_file(ctx, &USER_PROFILES, self.user_profile_path(value.user_id()), value.user_id().to_string(), value).await
    }
    async fn get_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile> {
        self.get_file(ctx, &USER_PROFILES, self.user_profile_path(id), id.to_string()).await
    }
    async fn update_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()> {
        self.update_file(ctx, &USER_PROFILES, self.user_profile_path(value.user_id()), value.user_id().to_string(), value).await
    }
    async fn delete_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.delete_file::<UserProfile>(ctx, &USER_PROFILES, self.user_profile_path(id)).await
    }
    async fn list_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>> {
        self.list_dir(ctx, &USER_PROFILES, self.user_profiles_dir(), query).await
    }
    async fn restore_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()> {
        self.restore_file::<UserProfile>(ctx, &USER_PROFILES, self.user_profile_path(id), id.to_string()).await
    }
    async fn purge_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.purge_file::<UserProfile>(ctx, &USER_PROFILES, self.user_profile_path(id)).await
    }
    async fn get_deleted_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile> {
        self.get_deleted_file(ctx, &USER_PROFILES, self.user_profile_path(id), id.to_string()).await
    }
    async fn list_deleted_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>> {
        self.list_deleted_dir(ctx, &USER_PROFILES, self.user_profiles_dir(), query).await
    }

    // Workspaces (issue 253) - the documents; their files lie in their
    // storage.
    async fn create_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()> {
        self.create_named_file(ctx, &WORKSPACES, self.workspace_path(value.id), value.id.to_string(), value).await
    }
    async fn get_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace> {
        self.get_file(ctx, &WORKSPACES, self.workspace_path(id), id.to_string()).await
    }
    async fn update_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()> {
        self.update_named_file(ctx, &WORKSPACES, self.workspace_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool> {
        self.delete_file::<Workspace>(ctx, &WORKSPACES, self.workspace_path(id)).await
    }
    async fn list_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>> {
        self.list_dir(ctx, &WORKSPACES, self.workspaces_dir(), query).await
    }
    async fn restore_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<()> {
        self.restore_named_file::<Workspace>(ctx, &WORKSPACES, self.workspace_path(id), id.to_string()).await
    }
    async fn purge_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool> {
        self.purge_file::<Workspace>(ctx, &WORKSPACES, self.workspace_path(id)).await
    }
    async fn get_deleted_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace> {
        self.get_deleted_file(ctx, &WORKSPACES, self.workspace_path(id), id.to_string()).await
    }
    async fn list_deleted_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>> {
        self.list_deleted_dir(ctx, &WORKSPACES, self.workspaces_dir(), query).await
    }
    async fn find_workspace_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Workspace> {
        self.find_dir(ctx, &WORKSPACES, self.workspaces_dir(), name).await
    }

    // Skills (issue 22)
    async fn create_skill(&self, ctx: &SecurityContext, value: &Skill) -> StoreResult<()> {
        self.create_named_file(ctx, &SKILLS, self.skill_path(value.id), value.id.to_string(), value).await
    }
    async fn get_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<Skill> {
        self.get_file(ctx, &SKILLS, self.skill_path(id), id.to_string()).await
    }
    async fn update_skill(&self, ctx: &SecurityContext, value: &Skill) -> StoreResult<()> {
        self.update_named_file(ctx, &SKILLS, self.skill_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<bool> {
        self.delete_file::<Skill>(ctx, &SKILLS, self.skill_path(id)).await
    }
    async fn list_skills(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Skill>> {
        self.list_dir(ctx, &SKILLS, self.skills_dir(), query).await
    }
    async fn restore_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<()> {
        self.restore_named_file::<Skill>(ctx, &SKILLS, self.skill_path(id), id.to_string()).await
    }
    async fn purge_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<bool> {
        self.purge_file::<Skill>(ctx, &SKILLS, self.skill_path(id)).await
    }
    async fn get_deleted_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<Skill> {
        self.get_deleted_file(ctx, &SKILLS, self.skill_path(id), id.to_string()).await
    }
    async fn list_deleted_skills(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Skill>> {
        self.list_deleted_dir(ctx, &SKILLS, self.skills_dir(), query).await
    }
    async fn find_skill_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Skill> {
        self.find_dir(ctx, &SKILLS, self.skills_dir(), name).await
    }

    // Artifacts (issue 271) - the documents; their content lies in their
    // storage.
    async fn create_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()> {
        self.create_named_file(ctx, &ARTIFACTS, self.artifact_path(value.id), value.id.to_string(), value).await
    }
    async fn get_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact> {
        self.get_file(ctx, &ARTIFACTS, self.artifact_path(id), id.to_string()).await
    }
    async fn update_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()> {
        self.update_named_file(ctx, &ARTIFACTS, self.artifact_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool> {
        self.delete_file::<Artifact>(ctx, &ARTIFACTS, self.artifact_path(id)).await
    }
    async fn list_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>> {
        self.list_dir(ctx, &ARTIFACTS, self.artifacts_dir(), query).await
    }
    async fn restore_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<()> {
        self.restore_named_file::<Artifact>(ctx, &ARTIFACTS, self.artifact_path(id), id.to_string()).await
    }
    async fn purge_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool> {
        self.purge_file::<Artifact>(ctx, &ARTIFACTS, self.artifact_path(id)).await
    }
    async fn get_deleted_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact> {
        self.get_deleted_file(ctx, &ARTIFACTS, self.artifact_path(id), id.to_string()).await
    }
    async fn list_deleted_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>> {
        self.list_deleted_dir(ctx, &ARTIFACTS, self.artifacts_dir(), query).await
    }
    async fn find_artifact_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Artifact> {
        self.find_dir(ctx, &ARTIFACTS, self.artifacts_dir(), name).await
    }
}

/// Preconfigured components, kept like the entities (issue 164).
#[async_trait]
impl ComponentStore for JsonFileStore {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.create_file(ctx, &COMPONENTS, self.component_path(value.id()), value.id().to_string(), value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.get_file(ctx, &COMPONENTS, self.component_path(id), id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.update_file(ctx, &COMPONENTS, self.component_path(value.id()), value.id().to_string(), value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.remove_file::<StoredComponents>(ctx, &COMPONENTS, self.component_path(id)).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.list_dir(ctx, &COMPONENTS, self.components_dir(), query).await
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
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
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
    use super::*;
    use chrono::Utc;
    use crate::security::SecurityAttributes;
    use crate::store::suite;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn store(dir: &std::path::Path) -> JsonFileStore {
        JsonFileStore::new(dir, Arc::new(crate::security::DummySecurityManager::new())).unwrap()
    }

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        let dir = tempdir().unwrap();
        suite::entity_store_behaves(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let dir = tempdir().unwrap();
        suite::queries_behave(&store(dir.path())).await;
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
        suite::deleting_marks(&store(dir.path())).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        let dir = tempdir().unwrap();
        suite::names_are_unique(&store(dir.path())).await;
        let dir = tempdir().unwrap();
        let owners = JsonFileStore::new(dir.path(), Arc::new(crate::security::OwnershipSecurityManager::new())).unwrap();
        suite::names_are_unique_beyond_what_is_read(&owners).await;
    }

    #[tokio::test]
    async fn a_document_stored_before_marks_existed_loads_and_one_not_deleted_is_written_without() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let system = SecurityContext::system();
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionId(Uuid::new_v4()), name: crate::value::name::Name::new("old").unwrap(), participants: Vec::new(), channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), created_at: Utc::now(), updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(), deleted: None,
        };
        store.create_session(&system, &session).await.unwrap();
        let text = tokio::fs::read_to_string(store.session_path(session.id)).await.unwrap();
        assert!(!text.contains("deleted"), "{text}");
        assert!(store.get_session(&system, session.id).await.unwrap().deleted.is_none());
    }

    #[tokio::test]
    async fn concurrent_writes_of_one_file_all_succeed_and_leave_no_temporary_files() {
        // Entities may be written at the same time - by two requests that
        // change one (issue 62); a state has one writer (issue 215).
        use crate::agent::{Personality, PersonalityId};
        let dir = tempdir().unwrap();
        let store = Arc::new(store(dir.path()));
        let system = SecurityContext::system();
        let personality = Personality {
            id: PersonalityId::new(), name: crate::value::name::ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, description: None, introduction: None,
            components: Default::default(), security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();

        let mut writes = tokio::task::JoinSet::new();
        for i in 0..16 {
            let (store, system) = (Arc::clone(&store), system.clone());
            let changed = Personality { description: Some(crate::value::text::Description::new(format!("version {i}")).unwrap()), ..personality.clone() };
            writes.spawn(async move { store.update_personality(&system, &changed).await });
        }
        while let Some(result) = writes.join_next().await {
            result.unwrap().expect("no writer loses its temporary file to another");
        }

        let path = store.personality_path(personality.id);
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [path.file_name().unwrap().to_string_lossy().into_owned()], "only the file itself");
        let stored = store.get_personality(&system, personality.id).await.expect("readable");
        assert!(stored.description.as_ref().map(|d| d.as_str()).unwrap_or_default().starts_with("version "), "one of the writes, whole");
    }

    #[tokio::test]
    async fn of_concurrent_writes_under_one_name_one_takes_it() {
        // Checked and written under the kind's lock (issue 250): nothing
        // writes between another's check and its write.
        use crate::actor::ActorId;
        use crate::agent::{Agent, Personality, PersonalityId};
        let dir = tempdir().unwrap();
        let store = Arc::new(store(dir.path()));
        let system = SecurityContext::system();
        let personality = Personality {
            id: PersonalityId::new(), name: crate::value::name::ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(), roles: None, description: None, introduction: None,
            components: Default::default(), security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };

        let mut writes = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let (store, system) = (Arc::clone(&store), system.clone());
            let agent = Agent::new(crate::value::name::ScopedName::new("Helper", Default::default()).unwrap(), &personality, ActorId::None);
            writes.spawn(async move { store.create_agent(&system, &agent).await });
        }
        let mut made = 0;
        while let Some(result) = writes.join_next().await {
            match result.unwrap() {
                Ok(()) => made += 1,
                Err(e) => assert!(matches!(e, StoreError::NameTaken { entity: "agent" }), "refused for its name: {e:?}"),
            }
        }
        assert_eq!(made, 1);
        assert_eq!(store.list_agents(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deleting_a_session_marks_its_file_and_purging_removes_it_leaving_its_state_to_the_state_store() {
        use crate::channel::{Channel, ChannelId};

        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let session_id = SessionId(Uuid::new_v4());
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            id: ChannelId(Uuid::new_v4()),
            session_id,
            name: crate::value::name::Name::new("general").unwrap(),
            components: crate::channel::moderation_components(None, Some("broadcast_policy")),
            classification: crate::confidentiality::Classification::CHANNEL_DEFAULT,
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
            name: crate::value::name::Name::new("to-delete").unwrap(),
            participants: Vec::new(),
            channels: vec![channel.id],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        store.create_session(&ctx, &session).await.unwrap();

        // Write out channel state so we can assert it gets removed.
        let agent_id = AgentId(Uuid::new_v4());
        store
            .put_agent_channel_state(agent_id, channel_id, &mut AgentChannelState::default())
            .await
            .unwrap();

        assert!(store.session_path(session_id).exists());
        assert!(store.agent_channel_state_dir(channel_id).exists());

        assert!(store.delete_session(&ctx, session_id).await.unwrap());
        assert!(store.session_path(session_id).exists(), "deleting marks the file");
        assert!(store.agent_channel_state_dir(channel_id).exists(), "the state is the state store's");
        assert!(store.purge_session(&ctx, session_id).await.unwrap());
        assert!(!store.session_path(session_id).exists(), "purging removes it");

        store.put_channel_state(channel_id, &mut ChannelState::new(channel_id)).await.unwrap();
        store.delete_channel_state(channel_id).await.unwrap();
        assert!(!store.channel_state_path(channel_id).exists());
        assert!(!store.agent_channel_state_dir(channel_id).exists());
        store.delete_channel_state(channel_id).await.expect("nothing to delete is no error");

        // Deleting or purging again reports that nothing was there.
        assert!(!store.delete_session(&ctx, session_id).await.unwrap());
        assert!(!store.purge_session(&ctx, session_id).await.unwrap());
        let err = store.get_session(&ctx, session_id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "session", .. }));
    }

    #[tokio::test]
    async fn list_sessions_skips_corrupted_files() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let s1 = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: crate::value::name::Name::new("session-1").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        store.create_session(&ctx, &s1).await.unwrap();

        // Create a corrupted JSON file
        let corrupted_path = store.sessions_dir().join("corrupted.json");
        fs::write(corrupted_path, "invalid json").unwrap();

        // This should NOT fail, it should return s1 and log a warning
        let sessions = store.list_sessions(&ctx, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, s1.id);
    }

    use crate::message::tests::{id, make_message};

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
        state.processing_state = crate::actor::ActorState::WaitingForUser;
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
            state.processing_state = crate::actor::ActorState::Idle;
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
        let folding = JsonFileStore::new(dir.path(), Arc::new(crate::security::DummySecurityManager::new())).unwrap().with_journal_floor(0);
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
