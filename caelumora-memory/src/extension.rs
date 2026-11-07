//! An agent's memory as its work reaches it (issue 518): what of it the work may recall where it
//! reads from (`MemoryReader`), and what a turn changes of it (`MemoryWriter`), each bound by the
//! plugin to every piece of work, which decides what may be recalled and checks every change as it
//! commits it.

use crate::memory::{Memory, MemoryChange, MemoryId};
use async_trait::async_trait;
use caelumora_api::staging::Committing;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::digest::Digest;
use caelumora_domain::store::StoreResult;

/// What of its agent's memory a work may recall where it reads from: a snapshot, taken when the
/// reader was made.
pub trait MemoryReader: Send + Sync {
    /// The entries the work may recall where it reads from, as the memory was then.
    fn recalled(&self) -> &[Memory];

    /// The digest of the memory as a whole then: the state of knowledge a work that recalls from
    /// it records.
    fn digest(&self) -> Option<Digest>;
}

/// What a turn changes of its agent's memory. The changes themselves the turn stages
/// (`Turn::stage`), and the plugin commits them when the turn completes, deriving where each was
/// made from the place of the work.
#[async_trait]
pub trait MemoryWriter: Committing<MemoryChange> {
    /// A number for a new entry, never handed out before: reserved at once, since the agent's
    /// turns elsewhere remember at the same time - the one exception to staging.
    ///
    /// # Errors
    /// If the memory cannot be read to count on from it.
    async fn reserve_id(&self) -> StoreResult<MemoryId>;

    /// Whether an entry may be reclassified as `classification` here, never above what the work
    /// may write: what a tool answers before the turn stages it, asked again as it is committed.
    async fn may_reclassify(&self, classification: &Classification) -> bool;
}
