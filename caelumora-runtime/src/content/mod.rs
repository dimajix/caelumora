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
//! (`ContentStorages`).

mod local;

use crate::backend::Backends;
use crate::terminal::ByteStream;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

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
    #[error("no storage of content named '{0}' is configured")]
    UnknownStorage(String),
    /// None of several storages is named for what is to be kept.
    #[error("which of the storages of content is meant must be named where there are several")]
    Unnamed,
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
    /// Removes everything whose key lies under `prefix` - whose leading
    /// segments it is; nothing there is no error.
    async fn remove_under(&self, prefix: &str) -> ContentResult<()>;
    /// What may be put into it: the classification what lies in it may
    /// have at most; `None` for no bound.
    fn destination(&self) -> Option<&Classification>;
}

/// The storages of content the configuration names, by their names - what
/// is kept there refers to its storage by that name. `UnknownStorage` and
/// `Unnamed` are its errors, never a storage's.
#[derive(Debug)]
pub struct ContentStorages(BTreeMap<String, Arc<dyn ContentStorage>>);

impl ContentStorages {
    pub(crate) fn new(storages: BTreeMap<String, Arc<dyn ContentStorage>>) -> Self {
        Self(storages)
    }

    /// The storage `name`.
    ///
    /// # Errors
    /// `UnknownStorage` if none of that name is configured.
    pub fn get(&self, name: &str) -> ContentResult<&Arc<dyn ContentStorage>> {
        self.0.get(name).ok_or_else(|| ContentError::UnknownStorage(name.to_string()))
    }

    /// The name of the storage an owner keeps new content in: `named`,
    /// which has to be one of them, or the one there is - `None` where none
    /// is configured.
    ///
    /// # Errors
    /// `UnknownStorage` if `named` is none of them, `Unnamed` if nothing is
    /// named among several.
    pub fn chosen(&self, named: Option<&str>) -> ContentResult<Option<&str>> {
        match named {
            Some(name) => self.0.get_key_value(name).map(|(name, _)| Some(name.as_str())).ok_or_else(|| ContentError::UnknownStorage(name.to_string())),
            None if self.0.len() > 1 => Err(ContentError::Unnamed),
            None => Ok(self.0.keys().next().map(String::as_str)),
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
    use local::LocalContentStorage;
    use std::path::PathBuf;

    fn storages(names: &[&str]) -> ContentStorages {
        ContentStorages::new(names.iter().map(|name| (name.to_string(), Arc::new(LocalContentStorage::new(PathBuf::from(*name), None)) as Arc<dyn ContentStorage>)).collect())
    }

    #[test]
    fn content_goes_to_the_storage_named_or_the_one_there_is() {
        assert!(matches!(storages(&["files"]).chosen(None), Ok(Some("files"))));
        assert!(matches!(storages(&[]).chosen(None), Ok(None)), "none configured");
        let several = storages(&["a", "b"]);
        assert!(matches!(several.chosen(None), Err(ContentError::Unnamed)), "which one, among several, must be said");
        assert!(matches!(several.chosen(Some("b")), Ok(Some("b"))));
        assert!(matches!(several.chosen(Some("c")), Err(ContentError::UnknownStorage(name)) if name == "c"), "not one that is not there");
        assert!(matches!(several.get("c"), Err(ContentError::UnknownStorage(name)) if name == "c"));
    }
}
