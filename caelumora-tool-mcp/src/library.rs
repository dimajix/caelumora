//! How the tools of MCP servers are joined to the kernel (issue 325): by their component kind
//! alone.

use crate::component;
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// The tools of MCP servers, as a library.
#[derive(Debug, Default)]
pub struct McpToolLibrary;

impl Library for McpToolLibrary {
    fn name(&self) -> &'static str {
        "tool_mcp"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        component::register(&mut library.components());
        Ok(())
    }
}
