//! An agent as the runtime works with it, apart from the actor that runs it
//! in a channel (`crate::actor::agent`, `crate::actor::external_agent`):
//!
//! - `state`: the one owner of an agent's `AgentState` per process;
//! - `work`: what both kinds of agent have of a piece of work - a turn, or
//!   an external agent's exchange with its remote agent;
//! - `turn`: the turn of an agent run here, the handle its actor and its
//!   components work through;
//! - `notes` and `memory`: the logic on top of the agent's notes and memory,
//!   read and changed within a turn.

pub mod memory;
pub mod notes;
pub mod state;
pub mod turn;
pub mod work;
