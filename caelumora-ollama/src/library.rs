//! How models served by Ollama are joined to the kernel (issue 329): by their component kind alone.
//! Whether it is the default model the program says, not the library
//! (`ComponentRegistry::set_default_kind`).

use crate::client;
use anyhow::Result;
use caelumora_kernel::library::{Library, LibraryBuilder};

/// Models served by Ollama, as a library.
#[derive(Debug, Default)]
pub struct OllamaLibrary;

impl Library for OllamaLibrary {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        client::register(&mut library.components());
        Ok(())
    }
}
