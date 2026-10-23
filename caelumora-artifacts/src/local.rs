//! The `local` artifact storage (issue 271): a directory per artifact under
//! the configured root, a file per version, on Caelumora's host - written
//! whole or not at all, as the files of a local workspace are.

use crate::artifact::ArtifactId;
use crate::manager::{ArtifactError, ArtifactResult, ArtifactStorage};
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::component::config_of_params;
use caelumora_runtime::terminal::{write_whole, ByteStream, WriteFailure};
use serde::Deserialize;
use std::io::{ErrorKind, Error};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio_util::io::ReaderStream;

/// The parameters of `local`: a directory per artifact under `root`, a
/// file per version, on Caelumora's host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalArtifactStorageConfig {
    root: PathBuf,
    /// What may be put into this storage: the classification an artifact
    /// in it may have at most; no bound unless given, since nothing then
    /// leaves the host.
    #[serde(default)]
    destination: Option<Classification>,
}

/// Registers the storage `local` of artifacts with `backends`.
pub(crate) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn ArtifactStorage, _>("local", |_, params, _| {
        let config: LocalArtifactStorageConfig = config_of_params(params)?;
        Ok(Arc::new(LocalArtifactStorage::new(config.root, config.destination)))
    });
}

#[derive(Debug)]
pub(crate) struct LocalArtifactStorage {
    root: PathBuf,
    destination: Option<Classification>,
}

impl LocalArtifactStorage {
    pub(crate) fn new(root: PathBuf, destination: Option<Classification>) -> Self {
        Self { root, destination }
    }

    fn dir(&self, artifact: ArtifactId) -> PathBuf {
        self.root.join(artifact.to_string())
    }
}

/// What a failure of the host's file system is, as an error says it: its
/// kind, never the path.
fn unavailable(error: Error) -> ArtifactError {
    ArtifactError::Unavailable(error.kind().to_string())
}

#[async_trait]
impl ArtifactStorage for LocalArtifactStorage {
    async fn put(&self, artifact: ArtifactId, version: u32, content: ByteStream, limit: u64) -> ArtifactResult<u64> {
        let dir = self.dir(artifact);
        fs::create_dir_all(&dir).await.map_err(unavailable)?;
        write_whole(&dir.join(version.to_string()), content, limit).await.map_err(|failure| match failure {
            WriteFailure::TooLarge => ArtifactError::TooLarge { limit },
            WriteFailure::BrokeOff(kind) => ArtifactError::Unavailable(format!("what was to be kept broke off: {kind}")),
            WriteFailure::Io(e) => unavailable(e),
        })
    }

    async fn get(&self, artifact: ArtifactId, version: u32) -> ArtifactResult<(u64, ByteStream)> {
        let path = self.dir(artifact).join(version.to_string());
        let file = fs::File::open(&path).await.map_err(|e| match e.kind() {
            ErrorKind::NotFound => ArtifactError::VersionNotFound { artifact, version },
            _ => unavailable(e),
        })?;
        let size = file.metadata().await.map_err(unavailable)?.len();
        Ok((size, Box::pin(ReaderStream::new(file))))
    }

    async fn delete(&self, artifact: ArtifactId, version: u32) -> ArtifactResult<()> {
        match fs::remove_file(self.dir(artifact).join(version.to_string())).await {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(unavailable(e)),
            _ => Ok(()),
        }
    }

    async fn remove(&self, artifact: ArtifactId) -> ArtifactResult<()> {
        match fs::remove_dir_all(self.dir(artifact)).await {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(unavailable(e)),
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
    use futures::StreamExt;
    use bytes::Bytes;
    use futures::stream;

    fn stream(content: &'static [u8]) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(content))]))
    }

    async fn read(storage: &LocalArtifactStorage, artifact: ArtifactId, version: u32) -> Vec<u8> {
        let (_, content) = storage.get(artifact, version).await.unwrap();
        content.collect::<Vec<_>>().await.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect()
    }

    #[tokio::test]
    async fn versions_are_kept_read_deleted_and_removed_with_their_artifact() {
        let root = tempfile::tempdir().unwrap();
        let storage = LocalArtifactStorage::new(root.path().to_path_buf(), None);
        let artifact = ArtifactId::new();
        assert_eq!(storage.put(artifact, 1, stream(b"first"), 100).await.unwrap(), 5);
        storage.put(artifact, 2, stream(b"second"), 100).await.unwrap();
        assert_eq!(read(&storage, artifact, 1).await, b"first");
        assert_eq!(read(&storage, artifact, 2).await, b"second");

        let too_large = storage.put(artifact, 3, stream(b"too large"), 4).await;
        assert!(matches!(too_large, Err(ArtifactError::TooLarge { limit: 4 })), "{too_large:?}");
        assert!(matches!(storage.get(artifact, 3).await, Err(ArtifactError::VersionNotFound { version: 3, .. })), "nothing left of it");

        storage.delete(artifact, 2).await.unwrap();
        assert!(matches!(storage.get(artifact, 2).await, Err(ArtifactError::VersionNotFound { .. })));
        storage.remove(artifact).await.unwrap();
        assert!(!root.path().join(artifact.to_string()).exists());
        storage.remove(artifact).await.expect("nothing there is no error");
    }
}
