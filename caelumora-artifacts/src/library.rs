//! Artifacts as a library (issue 516): the `artifact_tool` component, which publishes through the
//! `ArtifactPublisher` the plugin binds, and its texts.

use crate::component;
use crate::{ARTIFACTS, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_kernel::api::library::{Library, LibraryBuilder};

/// Artifacts, as a library: what of them an agent uses.
#[derive(Debug, Default)]
pub struct ArtifactsLibrary;

impl Library for ArtifactsLibrary {
    fn name(&self) -> &'static str {
        ARTIFACTS
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
