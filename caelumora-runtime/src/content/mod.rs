//! Where binary content lies (issue 282): the storages of content, each
//! served by one `ContentStorage`, which keeps bytes under keys and asks
//! nothing - who may read or change what a key names is the owner's of what
//! it names, an artifact's version the `ArtifactManager`'s. Each owner keeps
//! its content in a key space of its own, the first segment of its keys
//! (`artifacts/…`), so that one storage serves them all and removes what one
//! owner keeps of a thing by the prefix of its keys (`remove_under`).
//!
//! A storage is a backend (issue 324): made by the factory registered for
//! its type - `local` here (`register_backends`), others by a plugin, such as
//! an object store (issue 272) - from its parameters. The storages the
//! configuration names (`content.storage`) are made when the runtime is
//! built, once for the process, and handed to their owners by name
//! (`NamedBackends`).

mod local;

use crate::backend::Backends;
use crate::terminal::ByteStream;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::digest::{ContentDigest, Digest};
use futures::StreamExt;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;
use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    /// A key that is none: a path of segments, without `.`, `..` or an
    /// empty one - as its owner made it, never as a client wrote it.
    #[error("'{0}' is no key of content")]
    InvalidKey(String),
    #[error("nothing is kept under '{0}'")]
    NotFound(String),
    #[error("the content is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    /// What was kept is not as large as it was said to be: it changed while it was read, or broke
    /// off.
    #[error("the content was not as large as it was said to be")]
    Changed,
    /// The storage failed: what happened, never where.
    #[error("the storage of content failed: {0}")]
    Unavailable(String),
}

pub type ContentResult<T> = Result<T, ContentError>;

/// A storage of binary content (issue 282): bytes under keys, nothing of
/// permissions. A key is a path of segments separated by `/` - its first
/// the key space of its owner -, and what lies under a key is kept whole
/// or not at all. The service a storage of content is a backend of (issue
/// 324): the core brings `local`, a plugin may register another.
#[async_trait]
pub trait ContentStorage: Send + Sync + fmt::Debug {
    /// Keeps `content` under `key`, at most `limit` bytes, or nothing;
    /// answers how many bytes were kept.
    async fn put(&self, key: &str, content: ByteStream, limit: u64) -> ContentResult<u64>;
    /// What lies under `key`, with its size.
    async fn get(&self, key: &str) -> ContentResult<(u64, ByteStream)>;
    /// Removes what lies under `key`; nothing there is no error.
    async fn delete(&self, key: &str) -> ContentResult<()>;
    /// Puts a copy of what lies under `from` under `to`, as cheaply as the backend can (issue
    /// 282): two keys, each removed on its own.
    ///
    /// # Errors
    /// `NotFound` if nothing lies under `from`.
    async fn copy(&self, from: &str, to: &str) -> ContentResult<()>;
    /// Removes everything whose key lies under `prefix` - whose leading
    /// segments it is; nothing there is no error.
    async fn remove_under(&self, prefix: &str) -> ContentResult<()>;
    /// What may be put into it: the classification what lies in it may
    /// have at most; `None` for no bound.
    fn destination(&self) -> Option<&Classification>;
}

/// Keeps `content`, said to be `size` bytes, under `key` in `storage`, at most `limit` bytes, and
/// answers the digest of its bytes (`digest::content`), taken as they are kept - by an owner that
/// records what it keeps (issue 282). What is not as large as it was said to be is removed again.
///
/// # Errors
/// `TooLarge` for more than `limit` bytes, said or kept, `Changed` for other than `size`, and what
/// the storage reports.
pub async fn keep_digested(storage: &dyn ContentStorage, key: &str, size: u64, content: ByteStream, limit: u64) -> ContentResult<Digest> {
    if size > limit {
        return Err(ContentError::TooLarge { limit });
    }
    let digest = Arc::new(Mutex::new(ContentDigest::new(size)));
    let digesting = Arc::clone(&digest);
    let content: ByteStream = Box::pin(content.inspect(move |chunk| {
        if let Ok(chunk) = chunk {
            digesting.lock().unwrap_or_else(PoisonError::into_inner).update(chunk);
        }
    }));
    let kept = storage.put(key, content, limit).await?;
    match Arc::try_unwrap(digest) {
        Ok(digest) if kept == size => Ok(digest.into_inner().unwrap_or_else(PoisonError::into_inner).finish()),
        _ => {
            if let Err(e) = storage.delete(key).await {
                warn!("What was kept of content not as large as said stays: {e}");
            }
            Err(ContentError::Changed)
        }
    }
}

/// Registers the core's storages of content with `backends`: `local`.
pub(crate) fn register_backends(backends: &mut Backends) {
    local::register_backend(backends);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use caelumora_model::digest;
    use futures::stream;
    use local::LocalContentStorage;

    fn stream(content: &'static [u8]) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(content))]))
    }

    #[tokio::test]
    async fn content_is_kept_with_its_digest_or_not_at_all() {
        let root = tempfile::tempdir().unwrap();
        let storage = LocalContentStorage::new(root.path().to_path_buf(), None);
        assert_eq!(keep_digested(&storage, "a/1", 5, stream(b"hello"), 10).await.unwrap(), digest::content(b"hello"));
        assert!(matches!(keep_digested(&storage, "a/2", 4, stream(b"hello"), 10).await, Err(ContentError::Changed)), "not as large as said");
        assert!(matches!(storage.get("a/2").await, Err(ContentError::NotFound(_))), "removed again");
        assert!(matches!(keep_digested(&storage, "a/3", 11, stream(b"hello"), 10).await, Err(ContentError::TooLarge { limit: 10 })), "said too large");
    }
}
