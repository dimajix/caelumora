//! How external agents reached over A2A are joined to the runtime (issue
//! 326): by their component kind alone.

use crate::agent;
use anyhow::Result;
use caelumora_runtime::library::{Library, LibraryBuilder};

/// External agents reached over A2A, as a library.
#[derive(Debug, Default)]
pub struct A2aLibrary;

impl Library for A2aLibrary {
    fn name(&self) -> &'static str {
        "a2a"
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        agent::register(&mut library.components());
        Ok(())
    }
}
