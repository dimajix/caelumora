//! The `local` storage of content (issue 282): a file per key under the
//! configured root on Caelumora's host, a key's segments its directories -
//! written whole or not at all, as the files of a local workspace are.

use super::{ContentError, ContentResult, ContentStorage};
use crate::backend::Backends;
use crate::component::config_of_params;
use crate::terminal::{write_whole, ByteStream, WriteFailure};
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use serde::Deserialize;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio_util::io::ReaderStream;

/// The parameters of `local`: a file per key under `root`, on Caelumora's
/// host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalContentConfig {
    root: PathBuf,
    /// What may be put into this storage: the classification what lies in
    /// it may have at most; no bound unless given, since nothing then
    /// leaves the host.
    #[serde(default)]
    destination: Option<Classification>,
}

/// Registers the storage `local` of content with `backends`.
pub(super) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn ContentStorage, _>("local", |_, params, _| {
        let config: LocalContentConfig = config_of_params(params)?;
        Ok(Arc::new(LocalContentStorage::new(config.root, config.destination)))
    });
}

#[derive(Debug)]
pub(super) struct LocalContentStorage {
    root: PathBuf,
    destination: Option<Classification>,
}

impl LocalContentStorage {
    pub(super) fn new(root: PathBuf, destination: Option<Classification>) -> Self {
        Self { root, destination }
    }

    /// Where `key` lies under the root: its segments as directories, the
    /// last one a file.
    ///
    /// # Errors
    /// `InvalidKey` for one that would leave the root or name it.
    fn place(&self, key: &str) -> ContentResult<PathBuf> {
        if key.contains('\0') || key.split('/').any(|segment| matches!(segment, "" | "." | "..")) {
            return Err(ContentError::InvalidKey(key.to_string()));
        }
        Ok(key.split('/').fold(self.root.clone(), |path, segment| path.join(segment)))
    }
}

/// What a failure of the host's file system is, as an error says it: its
/// kind, never the path.
fn unavailable(error: Error) -> ContentError {
    ContentError::Unavailable(error.kind().to_string())
}

#[async_trait]
impl ContentStorage for LocalContentStorage {
    async fn put(&self, key: &str, content: ByteStream, limit: u64) -> ContentResult<u64> {
        let place = self.place(key)?;
        if let Some(dir) = place.parent() {
            fs::create_dir_all(dir).await.map_err(unavailable)?;
        }
        write_whole(&place, content, limit).await.map_err(|failure| match failure {
            WriteFailure::TooLarge => ContentError::TooLarge { limit },
            WriteFailure::BrokeOff(kind) => ContentError::Unavailable(format!("what was to be kept broke off: {kind}")),
            WriteFailure::Io(e) => unavailable(e),
        })
    }

    async fn get(&self, key: &str) -> ContentResult<(u64, ByteStream)> {
        let not_found = || ContentError::NotFound(key.to_string());
        let file = fs::File::open(self.place(key)?).await.map_err(|e| match e.kind() {
            ErrorKind::NotFound => not_found(),
            _ => unavailable(e),
        })?;
        let metadata = file.metadata().await.map_err(unavailable)?;
        if !metadata.is_file() {
            return Err(not_found());
        }
        Ok((metadata.len(), Box::pin(ReaderStream::new(file))))
    }

    async fn delete(&self, key: &str) -> ContentResult<()> {
        match fs::remove_file(self.place(key)?).await {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(unavailable(e)),
            _ => Ok(()),
        }
    }

    /// A hard link where the file system makes one, else a copy written as `put` writes, whole or
    /// not at all.
    async fn copy(&self, from: &str, to: &str) -> ContentResult<()> {
        let (source, place) = (self.place(from)?, self.place(to)?);
        if !fs::metadata(&source).await.is_ok_and(|metadata| metadata.is_file()) {
            return Err(ContentError::NotFound(from.to_string()));
        }
        if let Some(dir) = place.parent() {
            fs::create_dir_all(dir).await.map_err(unavailable)?;
        }
        if fs::hard_link(&source, &place).await.is_err() {
            let (size, content) = self.get(from).await?;
            self.put(to, content, size).await?;
        }
        Ok(())
    }

    async fn remove_under(&self, prefix: &str) -> ContentResult<()> {
        let dir = self.place(prefix)?;
        match fs::metadata(&dir).await {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&dir).await.map_err(unavailable),
            Err(e) if e.kind() != ErrorKind::NotFound => Err(unavailable(e)),
            // A key itself lies under no key.
            _ => Ok(()),
        }
    }

    fn destination(&self) -> Option<&Classification> {
        self.destination.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::{stream, StreamExt};

    fn stream(content: &'static [u8]) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(content))]))
    }

    async fn read(storage: &LocalContentStorage, key: &str) -> Vec<u8> {
        let (_, content) = storage.get(key).await.unwrap();
        content.collect::<Vec<_>>().await.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect()
    }

    #[tokio::test]
    async fn content_is_kept_copied_read_deleted_and_removed_under_a_prefix() {
        let root = tempfile::tempdir().unwrap();
        let storage = LocalContentStorage::new(root.path().to_path_buf(), None);
        assert_eq!(storage.put("artifacts/a/1", stream(b"first"), 100).await.unwrap(), 5);
        storage.put("artifacts/a/2", stream(b"second"), 100).await.unwrap();
        storage.put("artifacts/b/1", stream(b"other"), 100).await.unwrap();
        assert_eq!(read(&storage, "artifacts/a/1").await, b"first");
        assert_eq!(read(&storage, "artifacts/a/2").await, b"second");

        let too_large = storage.put("artifacts/a/3", stream(b"too large"), 4).await;
        assert!(matches!(too_large, Err(ContentError::TooLarge { limit: 4 })), "{too_large:?}");
        assert!(matches!(storage.get("artifacts/a/3").await, Err(ContentError::NotFound(_))), "nothing left of it");
        assert!(matches!(storage.get("artifacts/a").await, Err(ContentError::NotFound(_))), "a prefix is no key");

        storage.copy("artifacts/a/1", "artifacts/c/1").await.unwrap();
        assert_eq!(read(&storage, "artifacts/c/1").await, b"first", "a copy");
        assert!(matches!(storage.copy("artifacts/a/9", "artifacts/c/9").await, Err(ContentError::NotFound(_))), "of nothing");

        storage.delete("artifacts/a/2").await.unwrap();
        assert!(matches!(storage.get("artifacts/a/2").await, Err(ContentError::NotFound(_))));
        storage.delete("artifacts/a/2").await.expect("nothing there is no error");
        storage.remove_under("artifacts/a").await.unwrap();
        assert!(matches!(storage.get("artifacts/a/1").await, Err(ContentError::NotFound(_))));
        assert_eq!(read(&storage, "artifacts/b/1").await, b"other", "what lies under another prefix stays");
        assert_eq!(read(&storage, "artifacts/c/1").await, b"first", "a copy is removed on its own");
        storage.remove_under("artifacts/a").await.expect("nothing there is no error");
        storage.remove_under("artifacts/b/1").await.expect("a key itself lies under no key");
        assert_eq!(read(&storage, "artifacts/b/1").await, b"other");
    }

    #[tokio::test]
    async fn a_key_never_leaves_the_root() {
        let root = tempfile::tempdir().unwrap();
        let storage = LocalContentStorage::new(root.path().join("content"), None);
        for key in ["", "/etc/passwd", "../outside", "a/../../outside", "a//b", "a/", "./a", "a/./b", "a/.", "a\0b"] {
            assert!(matches!(storage.put(key, stream(b"x"), 10).await, Err(ContentError::InvalidKey(_))), "{key:?}");
            assert!(matches!(storage.remove_under(key).await, Err(ContentError::InvalidKey(_))), "{key:?}");
        }
        assert!(!root.path().join("outside").exists());
    }
}
