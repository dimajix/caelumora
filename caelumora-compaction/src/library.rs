//! How compaction is joined to the kernel (issue 464): its texts, the `compaction` component and
//! the strategies, and the strategy as a facet of which an agent has exactly one.

use crate::component;
use crate::summary_compaction::{self, SUMMARY_COMPACTION_KIND};
use crate::{CompactionStrategy, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_kernel::library::{Library, LibraryBuilder};

/// Compaction, as a library.
#[derive(Debug, Default)]
pub struct CompactionLibrary;

impl Library for CompactionLibrary {
    fn name(&self) -> &'static str {
        NAMESPACE
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        let components = &mut library.components();
        components.single::<dyn CompactionStrategy>(Some(SUMMARY_COMPACTION_KIND));
        component::register(components);
        summary_compaction::register(components);
        Ok(())
    }
}
