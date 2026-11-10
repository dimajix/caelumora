//! The documents of one kind as files (issue 446), where a `DocumentLayout` puts them and as a
//! `DocumentFormat` writes them, so that another backend over files can keep them in places and
//! forms of its own (issue 554).

use super::layout::DocumentLayout;
use super::{delete_if_exists, make_parent, to_backend_error, write_atomic};
use async_trait::async_trait;
use caelumora_domain::query::{ordered_after, FieldPath, Filter};
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::value::name::ScopedName;
use caelumora_security::Admission;
use caelumora_store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_store::Presence;
use futures::TryStreamExt;
use serde_json::Value;
use std::fmt::{self, Debug};
use std::fs::{self as std_fs, File, OpenOptions};
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};
use tokio::{fs, task};
use tracing::{debug, warn};
use uuid::Uuid;

/// How the documents of one kind are written in their files.
pub trait DocumentFormat: Debug + Send + Sync {
    /// The body of the document in the file at `path`, which holds `text`.
    ///
    /// # Errors
    /// If `text` holds no document.
    fn parse(&self, path: &Path, text: &str) -> StoreResult<Value>;

    /// The text of the file at `path` that holds `document`, in place of the text `previous` of the
    /// file of the document as it was, if it was there before.
    ///
    /// # Errors
    /// If the document cannot be written so.
    fn render(
        &self,
        path: &Path,
        document: &Document,
        previous: Option<&str>,
    ) -> StoreResult<String>;
}

/// A document as the JSON of its body.
#[derive(Debug)]
pub struct JsonFormat;

impl DocumentFormat for JsonFormat {
    fn parse(&self, _path: &Path, text: &str) -> StoreResult<Value> {
        serde_json::from_str(text).map_err(to_backend_error)
    }

    fn render(
        &self,
        _path: &Path,
        document: &Document,
        _previous: Option<&str>,
    ) -> StoreResult<String> {
        serde_json::to_string_pretty(&document.body).map_err(to_backend_error)
    }
}

/// What the file at `path` holds, `None` if it is gone.
///
/// # Errors
/// If it cannot be read.
async fn text_of(path: &Path) -> StoreResult<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// Opens the file at `path`, made with its directory if it is not there, and waits for its lock,
/// which closing it lets go.
fn locked(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std_fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    file.lock()?;
    Ok(file)
}

/// The documents of one kind as files, one per document, where its layout puts them and as its
/// format writes them. Every write holds the lock of the layout's file from reading what it judges,
/// the revision it writes in place of (issue 492) and the names of the others (issue 250), to
/// writing, so that no two writes miss each other and no two take one name, whichever program they
/// come from.
pub struct FileDocuments {
    kind: EntityKind,
    layout: Arc<dyn DocumentLayout>,
    format: Arc<dyn DocumentFormat>,
    /// What a document read is told by: its id, its name, its mark, its
    /// security attributes, its revision.
    document_of: DocumentOf,
    /// Held before the lock of the file, so that of this process one write of the kind at a time
    /// takes a thread waiting for it, whose thread the one that holds it may need.
    waiting: Mutex<()>,
}

impl Debug for FileDocuments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileDocuments")
            .field("kind", &self.kind)
            .field("layout", &self.layout)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl FileDocuments {
    /// The storage of the documents of `kind`, which lie where `layout` says, written as `format`
    /// writes them.
    pub fn new(
        kind: EntityKind,
        layout: Arc<dyn DocumentLayout>,
        format: Arc<dyn DocumentFormat>,
        document_of: DocumentOf,
    ) -> Self {
        Self { kind, layout, format, document_of, waiting: Mutex::new(()) }
    }

    /// Waits for the lock of the kind, on a thread of its own, since another program may hold it:
    /// held until what it answers is dropped.
    ///
    /// # Errors
    /// If it cannot be had.
    async fn writing(&self) -> StoreResult<(MutexGuard<'_, ()>, File)> {
        let waiting = self.waiting.lock().await;
        let lock = self.layout.lock();
        let file = task::spawn_blocking(move || locked(&lock))
            .await
            .map_err(to_backend_error)?
            .map_err(to_backend_error)?;
        Ok((waiting, file))
    }

    /// What the file at `path` holds as a document's body, `None` if it is gone or holds no
    /// document - which is logged: one broken file does not break a list.
    ///
    /// # Errors
    /// If the file cannot be read.
    async fn body(&self, path: &Path) -> StoreResult<Option<Value>> {
        let Some(text) = text_of(path).await? else { return Ok(None) };
        Ok(self.parsed(path, &text))
    }

    /// The body `text`, of the file at `path`, holds; `None` if it holds none, which is logged.
    fn parsed(&self, path: &Path, text: &str) -> Option<Value> {
        match self.format.parse(path, text) {
            Ok(body) => Some(body),
            Err(err) => {
                warn!("Failed to read the file {:?}: {}", path, err);
                None
            }
        }
    }

    /// The text of the file at `path` and the document it holds; `None` if there is none, or it
    /// cannot be read or told, which is logged.
    async fn stored(&self, path: &Path) -> Option<(String, Document)> {
        let text = match text_of(path).await {
            Ok(text) => text?,
            Err(err) => {
                warn!("Failed to read the file {:?}: {}", path, err);
                return None;
            }
        };
        match (self.document_of)(self.parsed(path, &text)?) {
            Ok(document) => Some((text, document)),
            Err(err) => {
                warn!("Failed to read the document {:?}: {}", path, err);
                None
            }
        }
    }

    /// The document at `path`, as `stored` reads it.
    async fn read(&self, path: &Path) -> Option<Document> {
        self.stored(path).await.map(|(_, document)| document)
    }

    /// Writes `text`, what `document` is rendered as, to `path`, and tells the layout, whose
    /// failure is only logged: the document is written all the same.
    async fn write(&self, path: &Path, text: &str, document: &Document) -> StoreResult<()> {
        write_atomic(path, text.as_bytes()).await?;
        if let Err(err) = self.layout.written(path, document).await {
            warn!("Failed to tell of the write of {:?}: {}", path, err);
        }
        Ok(())
    }

    /// `StoreError::Outdated` for the document `id` (issue 492).
    fn outdated(&self, id: Uuid) -> StoreError {
        StoreError::Outdated { entity: self.kind.name(), id: id.to_string() }
    }

    /// The text of the file at `path` and the document `id` it holds, if it is at the revision
    /// `read`: `StoreError::Outdated` otherwise, also if it cannot be read. Called while writing.
    async fn at_revision(
        &self,
        id: Uuid,
        path: &Path,
        read: Revision,
    ) -> StoreResult<(String, Document)> {
        let stored = self.stored(path).await;
        stored.filter(|(_, stored)| stored.revision == read).ok_or(self.outdated(id))
    }

    /// Refuses `document` if another document of the kind that is not deleted has its name (issue
    /// 250). Called while writing.
    async fn name_free(&self, document: &Document) -> StoreResult<()> {
        let Some(name) = named(document) else { return Ok(()) };
        let mut candidates = self.layout.named(name);
        while let Some((_, path)) = candidates.try_next().await? {
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
impl DocumentStorage for FileDocuments {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        let Some(path) = self.layout.find(id).await? else { return Ok(None) };
        let Some(text) = text_of(&path).await? else { return Ok(None) };
        self.format.parse(&path, &text).map(Some)
    }

    /// Reads the files in the order of their ids, each only when it is its turn, and judges
    /// `filter` on what they hold; whether one is marked deleted, and whether it is admitted, it
    /// leaves to the store, which would read it as its type to tell. A file that is no document is
    /// left out and logged. Every id is taken before the first file is read: what does not meet
    /// `filter` is passed over for what follows, so that how many are needed cannot be told.
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        _admission: Option<&Admission>,
        _presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        let documents = self.layout.all().try_collect::<Vec<_>>().await?;
        let mut found = Vec::new();
        for (id, path) in ordered_after(documents, after) {
            if found.len() >= limit {
                break;
            }
            let Some(body) = self.body(&path).await? else { continue };
            if filter.is_none_or(|filter| filter.matches(&body)) {
                found.push((id, body));
            }
        }
        Ok(found)
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        let mut candidates = self.layout.named(name);
        while let Some((_, path)) = candidates.try_next().await? {
            if let Some(document) = self.read(&path).await {
                if named(&document) == Some(name) {
                    return Ok(Some(document.body));
                }
            }
        }
        Ok(None)
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        let _writing = self.writing().await?;
        if self.layout.find(document.id).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "{} {} already exists",
                self.kind, document.id
            )));
        }
        self.name_free(&document).await?;
        let path = self.layout.place(&document).await?;
        let text = self.format.render(&path, &document, None)?;
        make_parent(&path).await?;
        debug!("Writing {} {} to {}", self.kind, document.id, path.display());
        self.write(&path, &text, &document).await
    }

    /// Reads the others of the kind only if `document` takes another name
    /// than it has as stored: whatever else changes takes nobody's name. Moved first, where its
    /// layout moves it, and then written: stopped between the two, it is one file. Rendered before
    /// either, so that what its format cannot write moves nothing.
    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        let _writing = self.writing().await?;
        let path = self.layout.find(document.id).await?.ok_or(self.outdated(document.id))?;
        let (previous, stored) = self.at_revision(document.id, &path, read).await?;
        if named(&document).is_some() && named(&stored) != named(&document) {
            self.name_free(&document).await?;
        }
        let target = self.layout.moved_to(&path, &document).await?;
        let text = self.format.render(&target, &document, Some(&previous))?;
        if target != path {
            make_parent(&target).await?;
            fs::rename(&path, &target).await.map_err(to_backend_error)?;
        }
        debug!("Writing {} {} to {}", self.kind, document.id, target.display());
        self.write(&target, &text, &document).await
    }

    /// What the layout tidies up after is logged where it fails: the document is gone all the
    /// same, and a second removal would find nothing to tidy up after.
    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        let _writing = self.writing().await?;
        let Some(path) = self.layout.find(id).await? else { return Ok(false) };
        self.at_revision(id, &path, read).await?;
        debug!("Purging {} at {}", self.kind, path.display());
        let removed = delete_if_exists(&path).await?;
        if let Err(err) = self.layout.removed(&path).await {
            warn!("Failed to tidy up after {:?}: {}", path, err);
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsonFileStore, DEFAULT_JOURNAL_FLOOR};
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::{Session, SessionId};
    use caelumora_domain::store::Stored;
    use caelumora_domain::value::name::Name;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::history::HistoryStore;
    use caelumora_store::transcript::TranscriptStore;
    use caelumora_store::{suite, EntityStores, Persistence, PersistenceBackend, StateStore};
    use futures::stream::{self, BoxStream};
    use futures::StreamExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::tempdir;

    /// What was asked of the layouts of a `NestedStore`.
    #[derive(Debug, Default)]
    struct Asked {
        placed: AtomicUsize,
        written: AtomicUsize,
        removed: AtomicUsize,
        /// Whether a write moves its document.
        moves: AtomicBool,
        /// Whether a name is looked for among no documents.
        narrows: AtomicBool,
        /// Whether being told of a write fails.
        written_fails: AtomicBool,
    }

    /// A layout that puts every document into a directory of its own, named by its id, and makes
    /// no directory itself, unlike that of the JSON store.
    #[derive(Debug)]
    struct Nested {
        dir: PathBuf,
        asked: Arc<Asked>,
    }

    /// A format that writes a line before the JSON of a document, to tell it was asked.
    #[derive(Debug)]
    struct Headed;

    impl DocumentFormat for Headed {
        fn parse(&self, path: &Path, text: &str) -> StoreResult<Value> {
            let json = text.strip_prefix("# headed\n").ok_or(StoreError::Backend(String::new()))?;
            JsonFormat.parse(path, json)
        }

        fn render(
            &self,
            path: &Path,
            document: &Document,
            previous: Option<&str>,
        ) -> StoreResult<String> {
            Ok(format!("# headed\n{}", JsonFormat.render(path, document, previous)?))
        }
    }

    impl Nested {
        fn path(&self, id: Uuid) -> PathBuf {
            self.dir.join(id.to_string()).join("document.json")
        }

        /// Where a moved document lies.
        fn moved(path: &Path) -> PathBuf {
            path.with_file_name("moved").join("document.json")
        }

        /// Where the document `id` is, moved or not.
        async fn located(&self, id: Uuid) -> StoreResult<PathBuf> {
            let moved = Self::moved(&self.path(id));
            let is_moved = fs::try_exists(&moved).await.map_err(to_backend_error)?;
            Ok(if is_moved { moved } else { self.path(id) })
        }

        async fn listed(&self) -> StoreResult<Vec<(Uuid, PathBuf)>> {
            let mut found = Vec::new();
            let mut entries = match fs::read_dir(&self.dir).await {
                Err(err) if err.kind() == ErrorKind::NotFound => return Ok(found),
                entries => entries.map_err(to_backend_error)?,
            };
            while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
                if let Ok(id) = entry.file_name().to_string_lossy().parse::<Uuid>() {
                    found.push((id, self.located(id).await?));
                }
            }
            Ok(found)
        }
    }

    #[async_trait]
    impl DocumentLayout for Nested {
        async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>> {
            let path = self.located(id).await?;
            Ok(fs::try_exists(&path).await.map_err(to_backend_error)?.then_some(path))
        }

        async fn place(&self, document: &Document) -> StoreResult<PathBuf> {
            self.asked.placed.fetch_add(1, Ordering::SeqCst);
            Ok(self.path(document.id))
        }

        fn all(&self) -> BoxStream<'_, StoreResult<(Uuid, PathBuf)>> {
            stream::once(self.listed())
                .map_ok(|found| stream::iter(found.into_iter().map(Ok)))
                .try_flatten()
                .boxed()
        }

        fn named<'a>(
            &'a self,
            _name: &'a ScopedName,
        ) -> BoxStream<'a, StoreResult<(Uuid, PathBuf)>> {
            match self.asked.narrows.load(Ordering::SeqCst) {
                true => stream::empty().boxed(),
                false => self.all(),
            }
        }

        fn lock(&self) -> PathBuf {
            self.dir.with_extension("lock")
        }

        async fn moved_to(&self, path: &Path, _document: &Document) -> StoreResult<PathBuf> {
            Ok(match self.asked.moves.load(Ordering::SeqCst) {
                false => path.to_path_buf(),
                true => Self::moved(path),
            })
        }

        async fn written(&self, _path: &Path, _document: &Document) -> StoreResult<()> {
            self.asked.written.fetch_add(1, Ordering::SeqCst);
            match self.asked.written_fails.load(Ordering::SeqCst) {
                true => Err(StoreError::Backend("not now".to_string())),
                false => Ok(()),
            }
        }

        async fn removed(&self, _path: &Path) -> StoreResult<()> {
            self.asked.removed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A backend whose documents lie in `Nested` layouts below `root`, its states in the files.
    #[derive(Debug)]
    struct NestedStore {
        root: PathBuf,
        files: Arc<JsonFileStore>,
        asked: Arc<Asked>,
    }

    impl PersistenceBackend for NestedStore {
        fn documents(
            &self,
            kind: EntityKind,
            document_of: DocumentOf,
        ) -> StoreResult<Arc<dyn DocumentStorage>> {
            let layout = Nested {
                dir: self.root.join("one").join("two").join(kind.name()),
                asked: Arc::clone(&self.asked),
            };
            Ok(Arc::new(FileDocuments::new(kind, Arc::new(layout), Arc::new(Headed), document_of)))
        }

        fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
            Arc::clone(&self.files).state_store()
        }

        fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
            Arc::clone(&self.files).history_store()
        }

        fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
            Arc::clone(&self.files).transcript_store()
        }
    }

    /// The backend below `root`.
    fn nested(root: &Path) -> Arc<NestedStore> {
        Arc::new(NestedStore {
            root: root.to_path_buf(),
            files: Arc::new(JsonFileStore::new(root, DEFAULT_JOURNAL_FLOOR).unwrap()),
            asked: Default::default(),
        })
    }

    fn stores(backend: &Arc<NestedStore>) -> EntityStores {
        let persistence = Persistence::over(Arc::<NestedStore>::clone(backend)).unwrap();
        EntityStores::open(&persistence, &Policies::permitting_all()).unwrap()
    }

    #[tokio::test]
    async fn a_layout_of_its_own_keeps_the_documents_as_well() {
        let dir = tempdir().unwrap();
        let backend = nested(dir.path());
        suite::entity_store_behaves(&stores(&backend)).await;
        suite::names_are_unique(&stores(&nested(tempdir().unwrap().path()))).await;
        suite::deleting_marks(&stores(&nested(tempdir().unwrap().path()))).await;
        suite::writes_are_judged_on_their_revision(&stores(&backend)).await;
        suite::storage_judges_writes_on_their_revision(backend.as_ref()).await;
        suite::purges_are_begun(&stores(&nested(tempdir().unwrap().path()))).await;
    }

    #[tokio::test]
    async fn a_document_lies_where_its_layout_says_and_is_tidied_up_after() {
        let dir = tempdir().unwrap();
        let backend = nested(dir.path());
        let system = SecurityContext::system();
        let sessions = stores(&backend).sessions;
        let id = SessionId::new();
        sessions.create(&system, &mut bare_session(id)).await.unwrap();

        let path = dir
            .path()
            .join("one/two")
            .join(Session::KIND.name())
            .join(id.to_string())
            .join("document.json");
        assert!(path.exists(), "its directories made by the write, not by the layout");

        sessions.delete(&system, id, &Removal::now(system.caller_id)).await.unwrap();
        assert_eq!(
            backend.asked.placed.load(Ordering::SeqCst),
            1,
            "placed once, and written in place"
        );
        assert_eq!(backend.asked.removed.load(Ordering::SeqCst), 0, "marked, it stays");
        assert!(sessions.purge(&system, id).await.unwrap());
        assert!(!path.exists());
        assert_eq!(backend.asked.removed.load(Ordering::SeqCst), 1, "told once it is gone");
    }

    #[tokio::test]
    async fn a_document_is_written_in_its_format_and_the_layout_told() {
        let dir = tempdir().unwrap();
        let backend = nested(dir.path());
        let system = SecurityContext::system();
        let sessions = stores(&backend).sessions;
        let id = SessionId::new();
        let mut session = bare_session(id);
        sessions.create(&system, &mut session).await.unwrap();

        let path = dir.path().join("one/two").join(Session::KIND.name()).join(id.to_string());
        let text = fs::read_to_string(path.join("document.json")).await.unwrap();
        assert!(text.starts_with("# headed\n"), "{text}");
        assert_eq!(sessions.get(&system, id).await.unwrap().id, id, "and read in it");
        assert_eq!(backend.asked.written.load(Ordering::SeqCst), 1);

        backend.asked.moves.store(true, Ordering::SeqCst);
        session.name = Name::new("moved").unwrap();
        sessions.update(&system, &mut session).await.unwrap();
        let moved = path.join("moved").join("document.json");
        assert!(!path.join("document.json").exists(), "moved");
        assert!(moved.is_file(), "where the layout says, its directories made");
        assert_eq!(sessions.get(&system, id).await.unwrap().name, "moved", "written there");
        assert_eq!(backend.asked.written.load(Ordering::SeqCst), 2);

        session.name = Name::new("again").unwrap();
        sessions.update(&system, &mut session).await.expect("it is still found");
    }

    /// A layout that cannot be told of a write leaves it written.
    #[tokio::test]
    async fn a_write_stands_though_telling_of_it_fails() {
        let dir = tempdir().unwrap();
        let backend = nested(dir.path());
        backend.asked.written_fails.store(true, Ordering::SeqCst);
        let system = SecurityContext::system();
        let sessions = stores(&backend).sessions;
        let id = SessionId::new();
        sessions.create(&system, &mut bare_session(id)).await.expect("written all the same");
        assert!(sessions.get(&system, id).await.is_ok());
    }

    /// What a name is looked for among is the layout's to narrow, for a lookup and for what is
    /// made alike.
    #[tokio::test]
    async fn a_name_is_looked_for_where_the_layout_says() {
        let dir = tempdir().unwrap();
        let backend = nested(dir.path());
        let system = SecurityContext::system();
        let personalities = stores(&backend).personalities;
        let mut first = make_personality("plain");
        personalities.create(&system, &mut first).await.unwrap();
        assert!(personalities.find_by_name(&system, &first.name).await.is_ok());
        let mut second = make_personality("plain");
        assert!(matches!(
            personalities.create(&system, &mut second).await,
            Err(StoreError::NameTaken { .. })
        ));

        backend.asked.narrows.store(true, Ordering::SeqCst);
        assert!(matches!(
            personalities.find_by_name(&system, &first.name).await,
            Err(StoreError::NotFound { .. })
        ));
        personalities.create(&system, &mut second).await.expect("no other is looked at");
    }
}
