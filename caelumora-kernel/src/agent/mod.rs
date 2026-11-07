//! An agent as the kernel works with it, apart from the actor that runs it in a channel
//! (`crate::actor::agent`, `crate::actor::external_agent`):
//!
//! - `manager`: the one owner of agents' lifecycle - deleting, restoring
//!   and purging them;
//! - `AgentMember`: what a plugin keeps behind an agent, which the manager
//!   removes as it purges the agent.

mod manager;

pub use manager::AgentManager;

use async_trait::async_trait;
use caelumora_domain::agent::{Agent, AgentId};
use caelumora_domain::store::{StoreError, StoreResult};
use caelumora_domain::value::name::Name;
use caelumora_domain::value::ValueError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No such agent, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no agent {0}")]
    NotFound(AgentId),
    /// The personality of an agent lies in a session's or a channel's scope
    /// the agent would not lie in (issue 178).
    #[error(
        "the personality '{0}' cannot make an agent outside the session or channel it lies in"
    )]
    PersonalityOutOfScope(Name),
    /// What a value was given as does not fit it: the name of an agent to
    /// make.
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type AgentResult<T> = Result<T, AgentError>;

/// What a plugin keeps behind an agent (`KernelBuilder::agent_member`, issue 518): removed when
/// the agent is purged, after the kernel's own state of it, as `ScopeMember::remove_behind` removes
/// what lies behind an entity of a scope.
#[async_trait]
pub trait AgentMember: Send + Sync {
    /// Removes what is kept behind `agent`, whose purge is begun: `AgentManager` gives it the agent
    /// between `EntityStore::begin_purge` and `EntityStore::purge`, so that if it fails, the agent
    /// stays deleted and is purged when it is purged again. Nothing to remove is not an error.
    ///
    /// # Errors
    /// Whatever its store reports; as `StoreError::Backend` a failure of what else it keeps.
    async fn remove_behind(&self, agent: &Agent) -> StoreResult<()>;
}

/// `error` of the store about agent `id`: not found is one case with not
/// readable.
fn store_error(id: AgentId) -> impl FnOnce(StoreError) -> AgentError {
    move |error| match error {
        StoreError::NotFound { .. } => AgentError::NotFound(id),
        error => AgentError::Store(error),
    }
}
