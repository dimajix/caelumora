//! What a plugin keeps behind an entity of the kernel's, which goes with it (issues 518, 607):
//! removed when the entity is purged, after the kernel's own state of it, by the manager that
//! purges it - an agent's memory with the agent.
//!
//! A plugin registers what it keeps behind the entities of a kind (`KernelBuilder::keeps_behind`)
//! as a `KeptBehind` of that kind; the kinds are those that `KeepsBehind`, whose manager removes
//! it, and the builder hands each manager those of its kind as it makes it.

use async_trait::async_trait;
use caelumora_domain::agent::Agent;
use caelumora_domain::channel::Channel;
use caelumora_domain::store::{StoreResult, Stored};

/// A kind of the kernel's that something may be kept behind: one whose purge removes it - an
/// agent, by the `AgentManager`, and a channel, by the `SessionManager` as its session is purged.
pub trait KeepsBehind: Stored {}

impl KeepsBehind for Agent {}

impl KeepsBehind for Channel {}

/// What a plugin keeps behind the entities of kind `T` (`KernelBuilder::keeps_behind`): removed
/// when one is purged, after the kernel's own state of it, as `ScopeMember::remove_behind` removes
/// what lies behind an entity of a scope.
#[async_trait]
pub trait KeptBehind<T: KeepsBehind>: Send + Sync {
    /// Removes what is kept behind the entity `id`, whose purge - or that of what holds it, a
    /// channel's session - is begun: if it fails, that stays deleted, and the next purge calls it
    /// again. Nothing to remove is no error.
    ///
    /// # Errors
    /// Whatever its store reports; as `StoreError::Backend` a failure of what else it keeps.
    async fn remove_behind(&self, id: T::Id) -> StoreResult<()>;
}
