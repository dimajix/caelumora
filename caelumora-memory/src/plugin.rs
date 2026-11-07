//! How an agent's memory is joined to the kernel (issue 518): the store of its kind, the
//! `MemoryManager` - announcing on the event bus - as what is kept behind every agent, and the
//! `MemoryReader` and the `MemoryWriter` bound to the views of an agent's work - the writer
//! committing what a turn staged of the memory.

use crate::extension::{MemoryReader, MemoryWriter};
use crate::manager::MemoryManager;
use crate::memory::{AgentMemory, MemoryChange};
use crate::view::{MemoryUse, RecalledMemory};
use crate::MEMORY;
use anyhow::Result;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use std::sync::Arc;

/// An agent's memory, as a plugin.
#[derive(Debug, Default)]
pub struct MemoryPlugin;

impl Plugin for MemoryPlugin {
    fn name(&self) -> &'static str {
        MEMORY
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let memories = kernel.entity_store::<AgentMemory>()?;
        let manager = Arc::new(MemoryManager::new(memories, Arc::clone(kernel.event_bus())));
        kernel.agent_member(Arc::clone(&manager));
        let reading = Arc::clone(&manager);
        kernel.extend_readers::<dyn MemoryReader, _, _>(move |session| {
            let manager = Arc::clone(&reading);
            async move {
                let recalled = RecalledMemory::of_work(&session, &manager).await?;
                Some(Arc::new(recalled) as Arc<dyn MemoryReader>)
            }
        })?;
        kernel.extend_writers::<dyn MemoryWriter, _, _>(move |session| {
            let manager = Arc::clone(&manager);
            async move {
                let used = MemoryUse::of_work(session, manager)?;
                Some(Arc::new(used) as Arc<dyn MemoryWriter>)
            }
        })?;
        kernel.commits::<MemoryChange, dyn MemoryWriter>()?;
        Ok(())
    }
}
