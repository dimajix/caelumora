//! The tools of MCP servers (issue 325): the component kind `mcp_server` (`component`), which
//! offers an agent the tools of one remote MCP server, and the library that registers it
//! (`library`). A crate of its own, so that a program that reaches no MCP server need not build
//! `rmcp`.

pub mod component;
pub mod library;
