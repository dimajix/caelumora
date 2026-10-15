//! An agent as the runtime works with it, apart from the actor that runs it
//! in a channel (`crate::actor::agent`, `crate::actor::external_agent`):
//!
//! - `manager`: the one owner of agents' lifecycle - deleting, restoring
//!   and purging them;
//! - `state`: the one owner of an agent's `AgentState` per process;
//! - `work`: what both kinds of agent have of a piece of work - a turn, or
//!   an external agent's exchange with its remote agent;
//! - `turn`: the turn of an agent run here, the handle its actor and its
//!   components work through;
//! - `notes` and `memory`: the logic on top of the agent's notes and memory,
//!   read and changed within a turn.

mod manager;
pub mod state;

pub use manager::AgentManager;

use caelumora_model::agent::AgentId;
use caelumora_model::store::StoreError;
use caelumora_model::value::name::Name;
use caelumora_model::value::ValueError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No such agent, or not one the caller may read - one case, so that
    /// nothing can be probed.
    #[error("there is no agent {0}")]
    NotFound(AgentId),
    /// The personality of an agent lies in a session's or a channel's scope
    /// the agent would not lie in (issue 178).
    #[error("the personality '{0}' cannot make an agent outside the session or channel it lies in")]
    PersonalityOutOfScope(Name),
    /// What a value was given as does not fit it: the name of an agent to
    /// make.
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type AgentResult<T> = Result<T, AgentError>;

/// `error` of the store about agent `id`: not found is one case with not
/// readable.
fn store_error(id: AgentId) -> impl FnOnce(StoreError) -> AgentError {
    move |error| match error {
        StoreError::NotFound { .. } => AgentError::NotFound(id),
        error => AgentError::Store(error),
    }
}
