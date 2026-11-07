//! An agent's memory as a library (issue 518): the `agent_memory` component, which reaches the
//! memory through the `MemoryReader` and the `MemoryWriter` the plugin binds, and its texts.

use crate::component;
use crate::{MEMORY, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// An agent's memory, as a library: what of it an agent uses.
#[derive(Debug, Default)]
pub struct MemoryLibrary;

impl Library for MemoryLibrary {
    fn name(&self) -> &'static str {
        MEMORY
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
