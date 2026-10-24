//! How compaction is joined to the runtime (issue 464): its texts, the `compaction` component and
//! the strategies, and the strategy as a facet of which an agent has exactly one.

use crate::component;
use crate::summary_compaction::{self, SUMMARY_COMPACTION_KIND};
use crate::{CompactionStrategy, NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;

/// Compaction, as a plugin.
#[derive(Debug, Default)]
pub struct CompactionPlugin;

impl Plugin for CompactionPlugin {
    fn name(&self) -> &'static str {
        NAMESPACE
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        let components = runtime.components();
        components.single::<dyn CompactionStrategy>(Some(SUMMARY_COMPACTION_KIND));
        component::register(components);
        summary_compaction::register(components);
        Ok(())
    }
}
