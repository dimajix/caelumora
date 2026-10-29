//! How the tools of MCP servers are joined to the runtime (issue 325): by
//! their component kind alone.

use crate::tool;
use anyhow::Result;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;

/// The tools of MCP servers, as a plugin.
#[derive(Debug, Default)]
pub struct McpPlugin;

impl Plugin for McpPlugin {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        tool::register(runtime.components());
        Ok(())
    }
}
