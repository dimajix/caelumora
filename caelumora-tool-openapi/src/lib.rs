//! The operations of servers described by OpenAPI (issue 252): the component kind
//! `openapi_server` (`component`), which offers an agent the chosen operations of one server as
//! tools, and the library that registers it (`library`). A crate of its own, as the tools of MCP
//! servers are, so that a program that reaches no such server need not build it.

pub mod component;
pub mod library;
