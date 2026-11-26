//! How agents' notes are joined to the kernel (issue 607): the store of their kind, the
//! `NotesManager` - announcing on the event bus - as a service, which the views reach them by, and
//! as what is kept behind every agent and every channel; the `NotesReader` and the `NotesWriter`
//! bound to the views of an agent's work - the writer committing what a turn staged of the notes -;
//! and the component that brings `/channel notes`.

use crate::command;
use crate::extension::{NotesReader, NotesWriter};
use crate::manager::NotesManager;
use crate::note::{AgentNotes, NoteChange};
use crate::view::{NotesRead, NotesUse};
use crate::NOTES;
use anyhow::Result;
use caelumora_domain::agent::Agent;
use caelumora_domain::channel::Channel;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use std::sync::Arc;

/// Agents' notes, as a plugin.
#[derive(Debug, Default)]
pub struct NotesPlugin;

impl Plugin for NotesPlugin {
    fn name(&self) -> &'static str {
        NOTES
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let notes = kernel.entity_store::<AgentNotes>()?;
        let manager = Arc::new(NotesManager::new(&notes, Arc::clone(kernel.event_bus())));
        kernel.provide(Arc::clone(&manager))?;
        kernel.keeps_behind::<Agent>(Arc::<NotesManager>::clone(&manager));
        kernel.keeps_behind::<Channel>(Arc::<NotesManager>::clone(&manager));
        let reading = Arc::clone(&manager);
        kernel.extend_readers::<dyn NotesReader, _, _>(move |session| {
            let manager = Arc::clone(&reading);
            async move {
                let read = NotesRead::of_work(session, manager).await?;
                Some(Arc::new(read) as Arc<dyn NotesReader>)
            }
        })?;
        kernel.extend_writers::<dyn NotesWriter, _, _>(move |session| {
            let manager = Arc::clone(&manager);
            async move {
                let used = NotesUse::of_work(session, manager)?;
                Some(Arc::new(used) as Arc<dyn NotesWriter>)
            }
        })?;
        kernel.commits::<NoteChange, dyn NotesWriter>()?;
        command::register(kernel.components());
        Ok(())
    }
}
