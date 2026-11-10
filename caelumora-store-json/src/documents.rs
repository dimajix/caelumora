//! The documents of one kind as JSON files (issue 446), where a `DocumentLayout` puts them, so
//! that another backend over files can keep them in places of its own (issue 554).

use super::layout::DocumentLayout;
use super::{delete_if_exists, make_parent, read_json_opt, to_backend_error, write_json_atomic};
use async_trait::async_trait;
use caelumora_domain::query::{ordered_after, FieldPath, Filter};
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::value::name::ScopedName;
use caelumora_security::Admission;
use caelumora_store::documents::{Document, DocumentOf, DocumentStorage};
use caelumora_store::Presence;
use serde_json::Value;
use std::fmt::{self, Debug};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, warn};
use uuid::Uuid;

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

/// The documents of one kind as JSON files, one per document, where its layout puts them. Every
/// write holds what the layout keeps writers apart by (`DocumentLayout::writing`) from reading what
/// it judges - the revision it writes in place of (issue 492), the names of the others (issue
/// 250) - to writing, so that no two writes miss each other and no two take one name. A document
/// stays where it was made.
pub struct JsonDocuments {
    kind: EntityKind,
    layout: Arc<dyn DocumentLayout>,
    /// What a document read is told by: its id, its name, its mark, its
    /// security attributes, its revision.
    document_of: DocumentOf,
}

impl Debug for JsonDocuments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonDocuments")
            .field("kind", &self.kind)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl JsonDocuments {
    /// The storage of the documents of `kind`, which lie where `layout` says.
    pub fn new(kind: EntityKind, layout: Arc<dyn DocumentLayout>, document_of: DocumentOf) -> Self {
        Self { kind, layout, document_of }
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

    /// `StoreError::Outdated` for the document `id` (issue 492).
    fn outdated(&self, id: Uuid) -> StoreError {
        StoreError::Outdated { entity: self.kind.name(), id: id.to_string() }
    }

    /// The document `id` at `path` as stored, if it is at the revision `read`:
    /// `StoreError::Outdated` otherwise, also if it cannot be read. Called while writing.
    async fn at_revision(&self, id: Uuid, path: &Path, read: Revision) -> StoreResult<Document> {
        self.read(path).await.filter(|stored| stored.revision == read).ok_or(self.outdated(id))
    }

    /// Refuses `document` if another document of the kind that is not deleted has its name (issue
    /// 250). Called while writing.
    async fn name_free(&self, document: &Document) -> StoreResult<()> {
        let Some(name) = named(document) else { return Ok(()) };
        for (_, path) in self.layout.all().await? {
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
        match self.layout.find(id).await? {
            Some(path) => read_json_opt(&path).await,
            None => Ok(None),
        }
    }

    /// Reads the files in the order of their ids, each only when it is its turn, and judges
    /// `filter` on what they hold; whether one is marked deleted, and whether it is admitted, it
    /// leaves to the store, which would read it as its type to tell. A file that is not JSON is
    /// left out and logged.
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        _admission: Option<&Admission>,
        _presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        let mut found = Vec::new();
        for (id, path) in ordered_after(self.layout.all().await?, after) {
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
        for (_, path) in self.layout.all().await? {
            if let Some(document) = self.read(&path).await {
                if named(&document) == Some(name) {
                    return Ok(Some(document.body));
                }
            }
        }
        Ok(None)
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        let _writing = self.layout.writing().await?;
        if self.layout.find(document.id).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "{} {} already exists",
                self.kind, document.id
            )));
        }
        self.name_free(&document).await?;
        let path = self.layout.place(&document).await?;
        make_parent(&path).await?;
        debug!("Writing {} {} to {}", self.kind, document.id, path.display());
        write_json_atomic(&path, &document.body).await
    }

    /// Reads the others of the kind only if `document` takes another name
    /// than it has as stored: whatever else changes takes nobody's name.
    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        let _writing = self.layout.writing().await?;
        let path = self.layout.find(document.id).await?.ok_or(self.outdated(document.id))?;
        let stored = self.at_revision(document.id, &path, read).await?;
        if named(&document).is_some() && named(&stored) != named(&document) {
            self.name_free(&document).await?;
        }
        debug!("Writing {} {} to {}", self.kind, document.id, path.display());
        write_json_atomic(&path, &document.body).await
    }

    /// What the layout tidies up after is logged where it fails: the document is gone all the
    /// same, and a second removal would find nothing to tidy up after.
    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        let _writing = self.layout.writing().await?;
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
    use caelumora_domain::removal::Removal;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::{Session, SessionId};
    use caelumora_domain::store::Stored;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::history::HistoryStore;
    use caelumora_store::transcript::TranscriptStore;
    use caelumora_store::{suite, EntityStores, Persistence, PersistenceBackend, StateStore};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    /// A layout that puts every document into a directory of its own, named by its id, and makes
    /// no directory itself, unlike that of the JSON store.
    #[derive(Debug)]
    struct Nested {
        dir: PathBuf,
        writes: Arc<Mutex<()>>,
        placed: Arc<AtomicUsize>,
        removed: Arc<AtomicUsize>,
    }

    impl Nested {
        fn path(&self, id: Uuid) -> PathBuf {
            self.dir.join(id.to_string()).join("document.json")
        }
    }

    #[async_trait]
    impl DocumentLayout for Nested {
        async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>> {
            let path = self.path(id);
            Ok(fs::try_exists(&path).await.map_err(to_backend_error)?.then_some(path))
        }

        async fn place(&self, document: &Document) -> StoreResult<PathBuf> {
            self.placed.fetch_add(1, Ordering::SeqCst);
            Ok(self.path(document.id))
        }

        async fn all(&self) -> StoreResult<Vec<(Uuid, PathBuf)>> {
            let mut found = Vec::new();
            let mut entries = match fs::read_dir(&self.dir).await {
                Err(err) if err.kind() == ErrorKind::NotFound => return Ok(found),
                entries => entries.map_err(to_backend_error)?,
            };
            while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
                if let Ok(id) = entry.file_name().to_string_lossy().parse::<Uuid>() {
                    found.push((id, self.path(id)));
                }
            }
            Ok(found)
        }

        async fn writing(&self) -> StoreResult<Box<dyn Send>> {
            Ok(Box::new(Arc::clone(&self.writes).lock_owned().await))
        }

        async fn removed(&self, _path: &Path) -> StoreResult<()> {
            self.removed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A backend whose documents lie in `Nested` layouts below `root`, its states in the files.
    #[derive(Debug)]
    struct NestedStore {
        root: PathBuf,
        files: Arc<JsonFileStore>,
        writes: Arc<Mutex<()>>,
        placed: Arc<AtomicUsize>,
        removed: Arc<AtomicUsize>,
    }

    impl PersistenceBackend for NestedStore {
        fn documents(
            &self,
            kind: EntityKind,
            document_of: DocumentOf,
        ) -> StoreResult<Arc<dyn DocumentStorage>> {
            let layout = Nested {
                dir: self.root.join("one").join("two").join(kind.name()),
                writes: Arc::clone(&self.writes),
                placed: Arc::clone(&self.placed),
                removed: Arc::clone(&self.removed),
            };
            Ok(Arc::new(JsonDocuments::new(kind, Arc::new(layout), document_of)))
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
            writes: Default::default(),
            placed: Default::default(),
            removed: Default::default(),
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
        assert_eq!(backend.placed.load(Ordering::SeqCst), 1, "placed once, and written in place");
        assert_eq!(backend.removed.load(Ordering::SeqCst), 0, "marked, it stays");
        assert!(sessions.purge(&system, id).await.unwrap());
        assert!(!path.exists());
        assert_eq!(backend.removed.load(Ordering::SeqCst), 1, "told once it is gone");
    }
}
