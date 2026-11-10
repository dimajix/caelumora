//! The content of a program of one person (issue 554): what is kept under a key lies with what the
//! key's second segment names - the attachments of a channel in `content` of the channel's
//! directory, the versions of an artifact in that of its project's.

use crate::places::Places;
use async_trait::async_trait;
use caelumora_api::component::Params;
use caelumora_api::error::ContentError;
use caelumora_api::terminal::ByteStream;
use caelumora_domain::confidentiality::Classification;
use caelumora_kernel::content::{ContentResult, ContentStorage, LocalContentStorage};
use caelumora_store_json::id_named;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The parameters of the content `home`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeContentConfig {
    /// What the program writes, as the stores `home` are told it.
    pub data_dir: PathBuf,
    /// The kind the second segment of a key names, by the key space it lies in: `attachments:
    /// channel`, `artifacts: project`. What lies in another lies in `content`.
    #[serde(default)]
    pub spaces: BTreeMap<String, String>,
    /// What may be put into it: the classification what lies in it may have at most; no bound
    /// unless given, since nothing then leaves the host.
    #[serde(default)]
    pub destination: Option<Classification>,
}

impl Params for HomeContentConfig {}

/// The content of a program of one person below its data directory: each key in `content` of the
/// directory of what its second segment names, where the stores `home` have it, apart from the
/// documents there, as `LocalContentStorage` keeps files. An error names the key it was given.
#[derive(Debug)]
pub struct HomeContent {
    places: Places,
    spaces: BTreeMap<String, String>,
    files: LocalContentStorage,
}

impl HomeContent {
    /// The content below the data directory of `config`.
    pub fn new(config: &HomeContentConfig) -> Self {
        let places = Places::new(config.data_dir.clone(), None, BTreeMap::new());
        let files = LocalContentStorage::new(config.data_dir.clone(), config.destination.clone());
        Self { places, spaces: config.spaces.clone(), files }
    }

    /// Where `key` lies, as a key of the files below the data directory: in `content` of the
    /// directory of what its second segment names, if its key space names a kind, or of the data
    /// directory.
    ///
    /// # Errors
    /// `Unavailable` if what it names cannot be looked for.
    async fn file_of(&self, key: &str) -> ContentResult<String> {
        let mut segments = key.splitn(3, '/');
        let (space, id, rest) = (segments.next(), segments.next(), segments.next());
        let named = space
            .zip(id.and_then(id_named))
            .and_then(|(space, id)| self.spaces.get(space).map(|kind| (space, kind, id)));
        let Some((space, kind, id)) = named else { return Ok(format!("content/{key}")) };
        let dir = self
            .places
            .directory_of_kind(kind, id)
            .await
            .map_err(|e| ContentError::Unavailable(e.to_string()))?;
        let below = dir
            .strip_prefix(self.places.data_dir())
            .map_err(|_| ContentError::Unavailable(format!("what {key} names lies apart")))?;
        let dir = below.to_string_lossy();
        Ok(match rest {
            Some(rest) => format!("{dir}/content/{space}/{rest}"),
            None => format!("{dir}/content/{space}"),
        })
    }
}

/// `error` of the files, as of the keys it was given for: each `(file, key)` of `keys`, so that it
/// names no place below the data directory.
fn as_given(error: ContentError, keys: &[(&str, &str)]) -> ContentError {
    let given = |file: String| {
        keys.iter().find(|(of, _)| *of == file).map_or(file, |(_, key)| (*key).to_string())
    };
    match error {
        ContentError::NotFound(file) => ContentError::NotFound(given(file)),
        ContentError::InvalidKey(file) => ContentError::InvalidKey(given(file)),
        error => error,
    }
}

#[async_trait]
impl ContentStorage for HomeContent {
    async fn put(&self, key: &str, content: ByteStream, limit: u64) -> ContentResult<u64> {
        let file = self.file_of(key).await?;
        self.files.put(&file, content, limit).await.map_err(|e| as_given(e, &[(&file, key)]))
    }

    async fn get(&self, key: &str) -> ContentResult<(u64, ByteStream)> {
        let file = self.file_of(key).await?;
        self.files.get(&file).await.map_err(|e| as_given(e, &[(&file, key)]))
    }

    async fn delete(&self, key: &str) -> ContentResult<()> {
        let file = self.file_of(key).await?;
        self.files.delete(&file).await.map_err(|e| as_given(e, &[(&file, key)]))
    }

    async fn copy(&self, from: &str, to: &str) -> ContentResult<()> {
        let (source, target) = (self.file_of(from).await?, self.file_of(to).await?);
        let keys = [(source.as_str(), from), (target.as_str(), to)];
        self.files.copy(&source, &target).await.map_err(|e| as_given(e, &keys))
    }

    async fn remove_under(&self, prefix: &str) -> ContentResult<()> {
        let file = self.file_of(prefix).await?;
        self.files.remove_under(&file).await.map_err(|e| as_given(e, &[(&file, prefix)]))
    }

    fn destination(&self) -> Option<&Classification> {
        self.files.destination()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HomeStore, HomeStoreConfig};
    use bytes::Bytes;
    use caelumora_domain::channel::testing::make_channel;
    use caelumora_domain::project::Project;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::{EntityStores, Persistence};
    use futures::{stream, StreamExt};
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use uuid::Uuid;

    /// The content below `dir`, the attachments with their channels, the artifacts with their
    /// projects.
    fn content(dir: &Path) -> HomeContent {
        let spaces = [("attachments", "channel"), ("artifacts", "project")]
            .map(|(space, kind)| (space.to_string(), kind.to_string()))
            .into();
        HomeContent::new(&HomeContentConfig {
            data_dir: dir.to_path_buf(),
            spaces,
            destination: None,
        })
    }

    /// The stores below `dir`.
    fn stores(dir: &Path) -> EntityStores {
        let config = HomeStoreConfig {
            config_dir: dir.join("config"),
            data_dir: dir.to_path_buf(),
            definitions: BTreeMap::new(),
            within: BTreeMap::new(),
        };
        let store = Persistence::over(Arc::new(HomeStore::new(&config).unwrap())).unwrap();
        EntityStores::open(&store, &Policies::permitting_all()).unwrap()
    }

    fn bytes(content: &'static [u8]) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(content))]))
    }

    async fn read(content: &HomeContent, key: &str) -> Vec<u8> {
        let (_, bytes) = content.get(key).await.unwrap();
        bytes.map(|chunk| chunk.unwrap().to_vec()).concat().await
    }

    #[tokio::test]
    async fn an_attachment_lies_with_its_channel() {
        let dir = tempdir().unwrap();
        let session = SessionId::new();
        let mut channel = make_channel(session, "main");
        stores(dir.path()).channels.create(&SecurityContext::system(), &mut channel).await.unwrap();
        let content = content(dir.path());
        let key = format!("attachments/{}/one", channel.id);

        assert_eq!(content.put(&key, bytes(b"hello"), 10).await.unwrap(), 5);

        let channel_dir = dir.path().join(format!("sessions/{session}/channels/{}", channel.id));
        assert!(channel_dir.join("content/attachments/one").is_file());
        assert_eq!(read(&content, &key).await, b"hello");
        let copy = format!("attachments/{}/two", channel.id);
        content.copy(&key, &copy).await.unwrap();
        assert_eq!(read(&content, &copy).await, b"hello");
        content.remove_under(&format!("attachments/{}", channel.id)).await.unwrap();
        assert!(
            !channel_dir.join("content/attachments").exists(),
            "every attachment of the channel"
        );
        assert!(channel_dir.join("channel.json").is_file(), "and nothing else");
    }

    #[tokio::test]
    async fn an_artifact_lies_with_its_project() {
        let dir = tempdir().unwrap();
        let session = SessionId::new();
        let mut own = Project::own_of(session, UserId::new());
        stores(dir.path()).projects.create(&SecurityContext::system(), &mut own).await.unwrap();
        let content = content(dir.path());
        let artifact = Uuid::new_v4();

        let project = dir.path().join(format!("sessions/{session}/projects/{}", own.id));
        // The artifact's document, where the stores put it.
        let document = project.join(format!("artifacts/{artifact}/artifact.json"));
        fs::create_dir_all(document.parent().unwrap()).unwrap();
        fs::write(&document, "{}").unwrap();
        let versions = format!("artifacts/{}/{artifact}", own.id);

        content.put(&format!("{versions}/v1"), bytes(b"a,b"), 10).await.unwrap();

        assert!(project.join(format!("content/artifacts/{artifact}/v1")).is_file());
        content.remove_under(&versions).await.unwrap();
        assert!(!project.join(format!("content/artifacts/{artifact}")).exists());
        assert!(document.is_file(), "its document is no content");
    }

    /// What lies in a key space no kind is named for, and what names no id, lies in `content`;
    /// what names an id without a directory where that would lie.
    #[tokio::test]
    async fn what_names_nothing_found_lies_apart() {
        let dir = tempdir().unwrap();
        let content = content(dir.path());
        let channel = Uuid::new_v4();

        content.put("other/a/b", bytes(b"x"), 10).await.unwrap();
        content.put("attachments/none/b", bytes(b"x"), 10).await.unwrap();
        content.put(&format!("attachments/{channel}/b"), bytes(b"x"), 10).await.unwrap();

        assert!(dir.path().join("content/other/a/b").is_file());
        assert!(dir.path().join("content/attachments/none/b").is_file());
        assert!(dir.path().join(format!("channels/{channel}/content/attachments/b")).is_file());
        let wrong = format!("attachments/{channel}/../x");
        assert!(
            matches!(content.get(&wrong).await, Err(ContentError::InvalidKey(key)) if key == wrong),
            "named by the key it was given"
        );
        assert!(matches!(
            content.get("other/missing").await,
            Err(ContentError::NotFound(key)) if key == "other/missing"
        ));
    }
}
