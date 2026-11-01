//! How the tools of MCP servers are joined to the kernel (issue 325): by their component kind
//! alone.

use crate::tool;
use anyhow::Result;
use caelumora_kernel::library::{Library, LibraryBuilder};

/// The tools of MCP servers, as a library.
#[derive(Debug, Default)]
pub struct McpLibrary;

impl Library for McpLibrary {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        tool::register(&mut library.components());
        Ok(())
    }
}
