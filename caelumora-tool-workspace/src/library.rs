//! How the tools of the workspace are joined to the kernel (issues 428, 532): their texts and the
//! component kind `workspace`.

use crate::{workspace, NAMESPACE, PROMPTS};
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
        workspace::register(&mut library.components());
        Ok(())
    }
}
