//! How the tool that generates images is joined to the kernel (issue 525): its texts and the
//! component kind `image_generation`.

use crate::{component, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// The tool that generates images, as a library.
#[derive(Debug, Default)]
pub struct ImageToolLibrary;

impl Library for ImageToolLibrary {
    fn name(&self) -> &'static str {
        "tool_image"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
