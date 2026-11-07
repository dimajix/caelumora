//! The built-in components that belong to no facet's own directory, one file per component kind
//! (issue 52). Tools, commands and prompt parts have modules of their own (`tool`, `command`,
//! `prompt`), language models crates of their own; a component that offers several facets - tools
//! and a prompt part, say - stays here. The facet traits live next to the code that uses them
//! (`crate::tool`, `crate::prompt`, `crate::message`), or in the API where a library or the kernel
//! provides them (`caelumora_api::command`, `caelumora_api::model`).

pub mod channel_history;
pub mod notes;
pub mod planning;
pub mod private_notes;

use caelumora_api::component::LibraryComponents;

/// Registers the component kinds of this directory with `registry`.
pub fn register_all(registry: &mut LibraryComponents<'_>) {
    channel_history::register(registry);
    planning::register(registry);
    notes::register(registry);
    private_notes::register(registry);
}
