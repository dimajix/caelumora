//! Where a backend that keeps its data in files on the host keeps what lies in a scope
//! (`StorePlaces`, issue 577): what the kernel tells an agent's work of where its session and its
//! channel are kept, for it to read there.

use async_trait::async_trait;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreResult;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

/// Where a backend keeps its data in the file system of the host (issue 577).
#[async_trait]
pub trait StorePlaces: Debug + Send + Sync {
    /// The directory it keeps its data below.
    fn data_dir(&self) -> &Path;

    /// The directory of what lies in `scope`.
    ///
    /// # Errors
    /// If a directory it looks into to find it cannot be read.
    async fn directory_of(&self, scope: &Scope) -> StoreResult<PathBuf>;
}
