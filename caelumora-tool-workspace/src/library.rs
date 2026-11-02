//! How the tools of the workspace are joined to the kernel (issue 428): by their component kinds
//! alone.

use crate::{terminal_tool, workspace_prompt_provider, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// The tools of the workspace and its prompt part, as a library.
#[derive(Debug, Default)]
pub struct WorkspaceToolLibrary;

impl Library for WorkspaceToolLibrary {
    fn name(&self) -> &'static str {
        "tool_workspace"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        terminal_tool::register(&mut library.components());
        workspace_prompt_provider::register(&mut library.components());
        Ok(())
    }
}
