//! Definitions as files named by their names (issue 554): a skill, a personality, a bundle or a
//! template is a file a person writes and keeps, in a directory of its kind, its name the file's,
//! its id its own or derived from its name.

use crate::places::{file_name_of, name_of_file, Places};
use async_trait::async_trait;
use caelumora_domain::config::parse_yaml;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::{EntityKind, StoreError, StoreResult};
use caelumora_domain::value::name::ScopedName;
use caelumora_store::documents::{Document, DocumentOf};
use caelumora_store_json::{entries_in, to_backend_error, DocumentFormat, DocumentLayout};
use futures::stream::{self, BoxStream};
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::warn;
use uuid::Uuid;

/// Where the ids of definitions without one of their own are derived in: a constant, so that a
/// name has the same id after every start and on every machine.
const NAMESPACE: Uuid = Uuid::from_u128(0x7c2e_4b1a_9d3f_4e8a_b5c6_1f0d_2a7e_9b34);

/// The directory below that of a kind's definitions the deleted ones lie in, by their ids, so that
/// their names are free.
const DELETED: &str = ".deleted";

/// How the definitions of one kind are written.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionFiles {
    /// Their directory, below the configuration directory, or below the directory of their scope.
    pub dir: String,
    /// The form of their files.
    pub format: FileFormat,
}

impl DefinitionFiles {
    /// Refuses a `dir` that is not one directory of a name of its own.
    ///
    /// # Errors
    /// `StoreError::Validation` for such a `dir`.
    pub(crate) fn check(&self, kind: &str) -> StoreResult<()> {
        let mut parts = Path::new(&self.dir).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(dir)), None) if dir != DELETED => Ok(()),
            _ => Err(StoreError::Validation(format!(
                "the definitions of {kind} lie in one directory of a name of their own"
            ))),
        }
    }
}

/// The form of a definition's file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FileFormat {
    /// Its fields in YAML.
    Yaml,
    /// Front matter, and the field `body` as the text after it: a text every definition of the
    /// kind has.
    Markdown { body: String },
}

impl FileFormat {
    fn extension(&self) -> &'static str {
        match self {
            FileFormat::Yaml => "yaml",
            FileFormat::Markdown { .. } => "md",
        }
    }
}

/// The id of the definition of `kind` named `name`, unless its file names another (issue 554): the
/// same for one name always, so that a pinned id shows a change of how names are folded.
pub(crate) fn derived_id(kind: EntityKind, name: &ScopedName) -> Uuid {
    Uuid::new_v5(&NAMESPACE, format!("{kind}\n{}", name.key()).as_bytes())
}

/// What a definition's file is to the backend: its id, and its name unless it is deleted.
#[derive(Deserialize)]
struct Head {
    id: Uuid,
    #[serde(flatten)]
    name: ScopedName,
    #[serde(default)]
    deleted: Option<Value>,
}

/// The directory of the definitions of a kind the file at `path` lies in: its directory, or the
/// one above for a deleted one.
fn kind_dir(path: &Path) -> Option<&Path> {
    let dir = path.parent()?;
    match dir.file_name() {
        Some(name) if name == DELETED => dir.parent(),
        _ => Some(dir),
    }
}

/// Whether the file at `path` is named by the name of its definition, not by its id.
fn named_by_name(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_none_or(|name| name != DELETED)
}

/// The fields of a definition in YAML.
fn fields_of(yaml: &str) -> StoreResult<Map<String, Value>> {
    if yaml.trim().is_empty() {
        return Ok(Map::new());
    }
    match parse_yaml::<Value>(yaml).map_err(to_backend_error)? {
        Value::Object(fields) => Ok(fields),
        _ => Err(StoreError::Backend("a definition is a mapping of its fields".to_string())),
    }
}

/// Front matter and the text after it, of a file in Markdown: none without front matter.
fn front_matter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else { return ("", text) };
    if let Some(after) = rest.strip_prefix("---\n") {
        return ("", after);
    }
    match rest.find("\n---\n") {
        Some(end) => (&rest[..=end], &rest[end + 5..]),
        None => ("", text),
    }
}

/// Whether two documents say the same of what a field is left out for.
type Same = fn(&Document, &Document) -> bool;

/// The fields `render` leaves out where reading the file without them gives the same, each with
/// how that is told: those a document says beside its body, of which an entity's other fields are
/// not, so that what reading without one gives cannot be told of them.
const LEFT_OUT: [(&str, Same); 2] = [
    ("revision", |one, other| one.revision == other.revision),
    ("security_attributes", |one, other| one.security_attributes == other.security_attributes),
];

/// What a file of a definition was found to be.
#[derive(Debug, Clone)]
struct Listed {
    id: Uuid,
    path: PathBuf,
    /// The key of its name, unless it is deleted.
    key: Option<String>,
}

/// The definitions of one kind as files named by their names: where they lie (`DocumentLayout`),
/// in the directory `dir` of the configuration directory for the global scope and of the directory
/// of their scope otherwise, and how they look in their files (`DocumentFormat`), which leave out
/// what the file and its place tell - the name, a scope its directory has, an id derived from its
/// name, what reading it without gives - and give it back when it is read. Two files of one id, or
/// of one name as names compare, are both left out and reported: which of them would win depends
/// on the order of a directory.
#[derive(Debug)]
pub(crate) struct Definitions {
    places: Arc<Places>,
    kind: EntityKind,
    dir: String,
    format: FileFormat,
    document_of: DocumentOf,
    lock: PathBuf,
}

impl Definitions {
    /// The definitions of `kind`, written as `files` says, told by `document_of`, a write holding
    /// the lock of the file `lock`.
    pub(crate) fn new(
        places: Arc<Places>,
        kind: EntityKind,
        files: &DefinitionFiles,
        document_of: DocumentOf,
        lock: PathBuf,
    ) -> Self {
        let (dir, format) = (files.dir.clone(), files.format.clone());
        Self { places, kind, dir, format, document_of, lock }
    }

    /// The fields the file holds as it is written, without what is given back.
    fn written_fields(&self, text: &str) -> StoreResult<Map<String, Value>> {
        match &self.format {
            FileFormat::Yaml => fields_of(text),
            FileFormat::Markdown { body } => {
                let (front, after) = front_matter(text);
                let mut fields = fields_of(front)?;
                fields.insert(body.clone(), Value::String(after.to_string()));
                Ok(fields)
            }
        }
    }

    /// The scope of the definitions in the directory of the kind at `dir`.
    fn scope_in(&self, dir: &Path) -> StoreResult<Scope> {
        dir.parent()
            .and_then(|holder| self.places.scope_of(holder))
            .ok_or(StoreError::Backend(format!("no {} lies in this directory", self.kind)))
    }

    /// The id and the name of the definition in the file at `path`; `None` for a file that is
    /// gone, or none, which is logged.
    async fn head(&self, path: &Path) -> Option<(Uuid, Option<String>)> {
        let text = match fs::read_to_string(path).await {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return None,
            Err(err) => {
                warn!("The file {:?} cannot be read: {}", path, err);
                return None;
            }
        };
        let head = self
            .parse(path, &text)
            .and_then(|body| serde_json::from_value::<Head>(body).map_err(to_backend_error));
        match head {
            Ok(head) => Some((head.id, head.deleted.is_none().then(|| head.name.key()))),
            Err(err) => {
                warn!("The file {:?} is no {}: {}", path, self.kind, err);
                None
            }
        }
    }

    /// Every definition of the kind, those of an id or a name another has left out and reported.
    async fn listed(&self) -> StoreResult<Vec<Listed>> {
        let extension = self.format.extension();
        let mut files = Vec::new();
        for holder in self.places.holders_of_definitions().await? {
            let dir = holder.join(&self.dir);
            for dir in [dir.join(DELETED), dir] {
                let mut entries = Box::pin(entries_in(dir));
                while let Some(entry) = entries.try_next().await? {
                    let path = entry.path();
                    if path.extension().is_some_and(|found| found == extension) {
                        files.push(path);
                    }
                }
            }
        }
        let mut listed = Vec::new();
        for path in files {
            if let Some((id, key)) = self.head(&path).await {
                listed.push(Listed { id, path, key });
            }
        }
        let mut ids: HashMap<Uuid, Vec<PathBuf>> = HashMap::new();
        let mut keys: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for entry in &listed {
            ids.entry(entry.id).or_default().push(entry.path.clone());
            if let Some(key) = &entry.key {
                keys.entry(key.clone()).or_default().push(entry.path.clone());
            }
        }
        for paths in ids.values().chain(keys.values()).filter(|paths| paths.len() > 1) {
            warn!("These files are one {} and are left out: {:?}", self.kind, paths);
        }
        listed.retain(|entry| {
            ids[&entry.id].len() == 1 && entry.key.as_ref().is_none_or(|key| keys[key].len() == 1)
        });
        Ok(listed)
    }

    /// Where the definition of `name` is written in the directory `dir` of its kind.
    fn path_of(&self, dir: &Path, name: &ScopedName) -> StoreResult<PathBuf> {
        let file = format!("{}.{}", file_name_of(name.as_str()), self.format.extension());
        if file.len() > 255 {
            return Err(StoreError::Validation(format!(
                "the name of the {} '{name}' is too long for a file",
                self.kind
            )));
        }
        Ok(dir.join(file))
    }

    /// Refuses `path` if something lies there.
    async fn free(&self, path: PathBuf) -> StoreResult<PathBuf> {
        match fs::try_exists(&path).await.map_err(to_backend_error)? {
            true => Err(StoreError::NameTaken { entity: self.kind.name() }),
            false => Ok(path),
        }
    }
}

impl DocumentFormat for Definitions {
    /// The name of a file named by it is the file's, whatever a field says.
    fn parse(&self, path: &Path, text: &str) -> StoreResult<Value> {
        let mut fields = self.written_fields(text)?;
        let dir =
            kind_dir(path).ok_or(StoreError::Backend("a file of no directory".to_string()))?;
        if !fields.contains_key("scope") {
            let scope = serde_json::to_value(self.scope_in(dir)?).map_err(to_backend_error)?;
            fields.insert("scope".to_string(), scope);
        }
        if named_by_name(path) {
            let stem = path.file_stem().and_then(|stem| stem.to_str());
            let name = stem.and_then(name_of_file);
            let name = name.ok_or(StoreError::Backend("no name of a definition".to_string()))?;
            fields.insert("name".to_string(), Value::String(name));
        }
        if !fields.contains_key("id") {
            let name: ScopedName =
                serde_json::from_value(Value::Object(fields.clone())).map_err(to_backend_error)?;
            let id = derived_id(self.kind, &name);
            fields.insert("id".to_string(), Value::String(id.to_string()));
        }
        Ok(Value::Object(fields))
    }

    /// An id the file held stays in it, also one its name derives (issue 554): it is what keeps a
    /// definition when its file is moved.
    fn render(&self, path: &Path, document: &Document) -> StoreResult<String> {
        let Value::Object(mut fields) = document.body.clone() else {
            return Err(StoreError::Backend("a definition is a mapping of its fields".to_string()));
        };
        let dir =
            kind_dir(path).ok_or(StoreError::Backend("a file of no directory".to_string()))?;
        let scope = serde_json::to_value(self.scope_in(dir)?).map_err(to_backend_error)?;
        if fields.get("scope") == Some(&scope) {
            fields.remove("scope");
        }
        let Some(name) = &document.name else {
            return Err(StoreError::Backend("a definition has a name".to_string()));
        };
        if named_by_name(path) {
            fields.remove("name");
        }
        let had_id = std_fs::read_to_string(path)
            .ok()
            .and_then(|text| self.written_fields(&text).ok())
            .is_some_and(|written| written.contains_key("id"));
        if !had_id && derived_id(self.kind, name) == document.id {
            fields.remove("id");
        }
        for (field, same) in LEFT_OUT {
            let mut without = document.body.clone();
            if let Value::Object(body) = &mut without {
                body.remove(field);
            }
            if (self.document_of)(without).is_ok_and(|without| same(&without, document)) {
                fields.remove(field);
            }
        }
        let yaml = |fields: &Map<String, Value>| match fields.is_empty() {
            true => Ok(String::new()),
            false => serde_saphyr::to_string(fields).map_err(to_backend_error),
        };
        match &self.format {
            FileFormat::Yaml => yaml(&fields),
            FileFormat::Markdown { body } => {
                let text = match fields.remove(body) {
                    Some(Value::String(text)) => text,
                    _ => return Err(StoreError::Backend(format!("its {body} is a text"))),
                };
                Ok(format!("---\n{}---\n{text}", yaml(&fields)?))
            }
        }
    }
}

#[async_trait]
impl DocumentLayout for Definitions {
    async fn find(&self, id: Uuid) -> StoreResult<Option<PathBuf>> {
        Ok(self.listed().await?.into_iter().find(|entry| entry.id == id).map(|entry| entry.path))
    }

    async fn place(&self, document: &Document) -> StoreResult<PathBuf> {
        let Some(name) = &document.name else {
            return Err(StoreError::Validation(format!("a {} has a name", self.kind)));
        };
        let dir = self.places.holder_of_definitions(&name.scope).await?.join(&self.dir);
        self.free(self.path_of(&dir, name)?).await
    }

    fn all(&self) -> BoxStream<'_, StoreResult<(Uuid, PathBuf)>> {
        stream::once(self.listed())
            .map_ok(|listed| {
                stream::iter(
                    listed.into_iter().map(|entry| Ok::<_, StoreError>((entry.id, entry.path))),
                )
            })
            .try_flatten()
            .boxed()
    }

    fn lock(&self) -> PathBuf {
        self.lock.clone()
    }

    /// A deleted definition by its id in `.deleted`, so that its name is free; one that is there by
    /// its name, also when it is renamed; in the directory it was made in, whatever its scope says.
    async fn moved_to(&self, path: &Path, document: &Document) -> StoreResult<PathBuf> {
        let dir =
            kind_dir(path).ok_or(StoreError::Backend("a file of no directory".to_string()))?;
        let target = match (&document.name, document.removal_id) {
            (_, Some(_)) => {
                let file = format!("{}.{}", document.id, self.format.extension());
                dir.join(DELETED).join(file)
            }
            (Some(name), None) => self.path_of(dir, name)?,
            (None, None) => {
                return Err(StoreError::Validation(format!("a {} has a name", self.kind)))
            }
        };
        if target == path {
            return Ok(target);
        }
        self.free(target).await
    }

    /// The directory of the deleted, once the last of them is gone.
    async fn removed(&self, path: &Path) -> StoreResult<()> {
        let Some(deleted) = path.parent().filter(|dir| !named_by_name(path) && dir.is_dir()) else {
            return Ok(());
        };
        match fs::remove_dir(deleted).await {
            Err(err)
                if !matches!(err.kind(), ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty) =>
            {
                Err(to_backend_error(err))
            }
            _ => Ok(()),
        }
    }
}
