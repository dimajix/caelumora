//! The tools of MCP servers (issue 325): the component kind `mcp_tool`,
//! which offers an agent the tools of one remote MCP server (`tool`), and
//! the plugin that registers it (`plugin`). A crate of its own, so that a
//! program that reaches no MCP server need not build `rmcp`.

pub mod plugin;
pub mod tool;
