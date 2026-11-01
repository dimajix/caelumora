//! Skills as a library (issue 516): the `skills` component, which reaches skills through the
//! `SkillReader` and the `SkillWriter` the plugin binds, and its texts.

use crate::component;
use crate::{NAMESPACE, PROMPTS, SKILLS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// Skills, as a library: what of them an agent uses.
#[derive(Debug, Default)]
pub struct SkillsLibrary;

impl Library for SkillsLibrary {
    fn name(&self) -> &'static str {
        SKILLS
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
