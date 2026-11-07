//! How the tools that reach the web are joined to the kernel (issue 544): their texts and the
//! component kinds `web_fetch` and `web_search`.

use crate::{component, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// The tools that reach the web, as a library.
#[derive(Debug, Default)]
pub struct WebToolLibrary;

impl Library for WebToolLibrary {
    fn name(&self) -> &'static str {
        "tool_web"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
