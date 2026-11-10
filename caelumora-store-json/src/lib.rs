//! The JSON store (issue 446): a backend of the stores that keeps every document and every state in
//! files below a directory (`JsonFileStore`), registered as the backend `json` by its plugin
//! (`JsonStorePlugin`). Its states (`JsonStates`) lie where a `StateLayout` puts them, so that
//! another backend over files can keep them in places of its own (issue 554).

mod chains;
mod histories;
mod layout;
pub mod plugin;
mod states;
mod transcripts;

pub use layout::StateLayout;
pub use states::{JsonStates, DEFAULT_JOURNAL_FLOOR};

use async_trait::async_trait;
use caelumora_domain::query::{ordered_after, FieldPath, Filter};
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::value::name::ScopedName;
use caelumora_security::Admission;
use caelumora_store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_store::history::HistoryStore;
use caelumora_store::transcript::TranscriptStore;
use caelumora_store::{PersistenceBackend, Presence, StateStore};
use layout::RuntimeLayout;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::{self, Debug, Display};
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

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
        JsonFileStore::new(&self.path, self.journal_floor.unwrap_or(DEFAULT_JOURNAL_FLOOR))
    }
}

/// One JSON file per document (`EntityStore`), and the states as `JsonStates` keeps them, in the
/// layout of this store (`RuntimeLayout`).
#[derive(Debug)]
pub struct JsonFileStore {
    root: PathBuf,
    states: Arc<JsonStates>,
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<JsonDocuments>>>,
}

impl JsonFileStore {
    /// A store of the files under `root`, a journal folded into its snapshot once it is
    /// `journal_floor` large, if that is more than the snapshot.
    ///
    /// # Errors
    /// If the directories of the states cannot be made.
    pub fn new(root: impl Into<PathBuf>, journal_floor: u64) -> StoreResult<Self> {
        let root = root.into();
        info!("Creating JSON file store at {:?}", root);
        let layout = Arc::new(RuntimeLayout::new(&root)?);
        let states = Arc::new(JsonStates::new(layout, journal_floor));
        Ok(Self { root, states, documents: Default::default() })
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

fn to_backend_error(err: impl Display) -> StoreError {
    StoreError::Backend(err.to_string())
}

/// Opens a kind in the directory of its name, `entities/<kind>`. The states are its `JsonStates`.
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
        Ok(Arc::<JsonStates>::clone(&self.states))
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Ok(Arc::<JsonStates>::clone(&self.states))
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Ok(Arc::<JsonStates>::clone(&self.states))
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::{ChannelId, ChannelState};
    use caelumora_domain::query::Query;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::{Session, SessionId};
    use caelumora_domain::store::{StoreError, Stored};
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_domain::value::text::Description;
    use caelumora_security::{OwnershipPolicy, Policies, SecurityContext};
    use caelumora_store::{suite, EntityStore, EntityStores, Persistence};
    use tempfile::tempdir;
    use tokio::task::JoinSet;

    fn store(dir: &Path) -> JsonFileStore {
        JsonFileStore::new(dir, DEFAULT_JOURNAL_FLOOR).unwrap()
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

    /// What it hands out are the state stores of its own files.
    #[tokio::test]
    async fn hands_out_the_stores_of_its_states_below_its_root() {
        let dir = tempdir().unwrap();
        let store = Arc::new(store(dir.path()));
        let states = Arc::clone(&store).state_store().unwrap();
        let histories = Arc::clone(&store).history_store().unwrap();
        let transcripts = store.transcript_store().unwrap();
        suite::state_store_behaves(states.as_ref()).await;
        suite::work_under_way_is_indexed(states.as_ref()).await;
        suite::history_store_behaves(states.as_ref(), histories.as_ref()).await;
        suite::transcript_store_behaves(states.as_ref(), transcripts.as_ref()).await;
        suite::given_store_behaves(states.as_ref(), transcripts.as_ref()).await;

        let channel = ChannelId::new();
        states.put_channel_state(channel, &mut ChannelState::new(channel)).await.unwrap();
        assert!(
            dir.path().join("runtime/channel_state").join(format!("{channel}.json")).exists(),
            "below its root"
        );
    }

    #[tokio::test]
    async fn folds_a_journal_from_the_floor_it_is_given() {
        let dir = tempdir().unwrap();
        let states = Arc::new(JsonFileStore::new(dir.path(), 0).unwrap()).state_store().unwrap();
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let journal = dir
            .path()
            .join("runtime/agent_channel_state")
            .join(channel.to_string())
            .join(format!("{agent}.jsonl"));
        let mut state = AgentChannelState::default();
        for _ in 0..2 {
            states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        }
        assert!(journal.exists(), "the second put goes to the journal");
        let mut puts = 0;
        while journal.exists() {
            states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
            puts += 1;
            assert!(puts < 1000, "the journal is never folded");
        }
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
        use caelumora_domain::agent::Personality;
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
        use caelumora_domain::actor::ActorId;
        use caelumora_domain::agent::Agent;
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
        use caelumora_domain::channel::testing::make_channel;

        let dir = tempdir().unwrap();
        let layout = RuntimeLayout::new(dir.path()).unwrap();
        let states = Arc::new(store(dir.path())).state_store().unwrap();
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
        states
            .put_agent_channel_state(agent_id, channel_id, &mut AgentChannelState::default())
            .await
            .unwrap();
        let agents = layout.agents_in(channel_id).await.unwrap();

        assert!(path_of::<Session>(dir.path(), &session_id).exists());
        assert!(agents.exists());

        assert!(sessions.delete(&ctx, session_id, &Removal::now(ctx.caller_id)).await.unwrap());
        assert!(path_of::<Session>(dir.path(), &session_id).exists(), "deleting marks the file");
        assert!(agents.exists(), "the state is the state store's");
        assert!(sessions.purge(&ctx, session_id).await.unwrap());
        assert!(!path_of::<Session>(dir.path(), &session_id).exists(), "purging removes it");

        states.put_channel_state(channel_id, &mut ChannelState::new(channel_id)).await.unwrap();
        states.delete_channel_state(channel_id).await.unwrap();
        assert!(!layout.channel_state(channel_id).await.unwrap().exists());
        assert!(!agents.exists());
        states.delete_channel_state(channel_id).await.expect("nothing to delete is no error");

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
}
