//! Sub-agents as a library (issue 558): the `subagents` component, which starts, asks and ends
//! sub-agents through the `SubAgentWriter` the plugin binds, and its texts.

use crate::component;
use crate::{NAMESPACE, PROMPTS, SUBAGENTS};
use anyhow::Result;
use caelumora_api::library::{Library, LibraryBuilder};

/// Sub-agents, as a library: what of them an agent uses.
#[derive(Debug, Default)]
pub struct SubagentsLibrary;

impl Library for SubagentsLibrary {
    fn name(&self) -> &'static str {
        SUBAGENTS
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        component::register(&mut library.components());
        Ok(())
    }
}
