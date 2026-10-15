//! What an agent does in a channel (issue 370): both kinds of agent - one
//! whose mind is a model run here (`actor::agent`), one whose mind runs
//! elsewhere (`actor::external_agent`) - with what they share (their body,
//! how they take in what they hear, the commands they carry out), the turn
//! of an agent run here (`turn`, `work`, `memory`, `notes`), and the facets
//! their components serve: tools (`tool`), prompt parts and contexts
//! (`prompt`), the model (`llm`), message stages (`message`), compaction
//! (`compaction`), an agent that runs elsewhere (`external`) and an agent's
//! commands (`command`), with the built-in kinds (`component_impl`). It runs
//! on the runtime (`caelumora-runtime`), which knows agents only as a kind
//! of participant, and joins it as a plugin (`plugin::AgentsPlugin`) - an
//! essential one: without it, the program is a chat between users.

pub mod actor;
pub mod command;
pub mod compaction;
pub mod component_impl;
pub mod error;
pub mod external;
pub mod llm;
pub mod memory;
pub mod message;
pub mod notes;
pub mod plugin;
pub mod prompt;
pub mod tool;
pub mod turn;
pub mod work;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod channel_tests;
