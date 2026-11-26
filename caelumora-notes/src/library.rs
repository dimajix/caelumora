//! An agent's notes as a library (issue 607): the components `notes` and `private_notes`, which
//! reach the notes through the `NotesReader` and the `NotesWriter` the plugin binds, and their
//! texts.

use crate::component;
use crate::{NAMESPACE, NOTES, PROMPTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// An agent's notes, as a library: what of them an agent uses.
#[derive(Debug, Default)]
pub struct NotesLibrary;

impl Library for NotesLibrary {
    fn name(&self) -> &'static str {
        NOTES
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
