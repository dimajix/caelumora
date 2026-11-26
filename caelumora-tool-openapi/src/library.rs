//! How the operations of servers described by OpenAPI are joined to the kernel (issue 252): by
//! their component kind alone.

use crate::component;
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// The operations of servers described by OpenAPI, as a library.
#[derive(Debug, Default)]
pub struct OpenApiToolLibrary;

impl Library for OpenApiToolLibrary {
    fn name(&self) -> &'static str {
        "tool_openapi"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        component::register(&mut library.components());
        Ok(())
    }
}
