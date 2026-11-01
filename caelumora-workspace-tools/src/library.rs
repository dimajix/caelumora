//! How the tools of the workspace are joined to the kernel (issue 428): by their component kinds
//! alone.

use crate::{terminal_tool, workspace_prompt_provider, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_kernel::library::{Library, LibraryBuilder};

/// The tools of the workspace and its prompt part, as a library.
#[derive(Debug, Default)]
pub struct WorkspaceToolsLibrary;

impl Library for WorkspaceToolsLibrary {
    fn name(&self) -> &'static str {
        "workspace_tools"
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        terminal_tool::register(&mut library.components());
        workspace_prompt_provider::register(&mut library.components());
        Ok(())
    }
}
