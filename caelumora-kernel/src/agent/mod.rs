//! An agent as the kernel works with it, apart from the actor that runs it in a channel
//! (`crate::actor::agent`, `crate::actor::external_agent`): `manager`, the one owner of agents'
//! lifecycle - making, deleting, restoring and purging them -, with what a caller asks for of a new
//! one (`spec`) and how it fails (`AgentError`).

mod manager;
mod spec;

pub use manager::AgentManager;
pub use spec::AgentSpec;

use caelumora_domain::agent::AgentId;
use caelumora_domain::store::StoreError;
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
    /// The agent of a name asked for lies where it is asked for, but with another personality than
    /// the one asked for (issue 178): neither reused nor replaced.
    #[error("the agent '{0}' already exists where it is asked for, with another personality")]
    OfAnotherPersonality(Name),
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
