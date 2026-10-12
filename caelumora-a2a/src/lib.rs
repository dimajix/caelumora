//! External agents reached over the Agent2Agent protocol (issue 326): the
//! component kind `a2a_agent`, which provides the runtime's `ExternalAgent`
//! facet for one remote A2A agent (`agent`), and the plugin that registers
//! it (`plugin`). A crate of its own, so that the core keeps the facet alone
//! and names no protocol.

pub mod agent;
pub mod plugin;
