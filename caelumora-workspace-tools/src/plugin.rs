//! How the tools of the workspace are joined to the runtime (issue 428): by
//! their component kinds alone.

use crate::{terminal_tool, workspace_prompt_provider, NAMESPACE, PROMPTS};
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use anyhow::Result;

/// The tools of the workspace and its prompt part, as a plugin.
#[derive(Debug, Default)]
pub struct WorkspaceToolsPlugin;

impl Plugin for WorkspaceToolsPlugin {
    fn name(&self) -> &'static str {
        "workspace_tools"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        terminal_tool::register(runtime.components());
        workspace_prompt_provider::register(runtime.components());
        Ok(())
    }
}
