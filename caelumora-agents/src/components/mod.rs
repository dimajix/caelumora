//! The built-in components that belong to no facet's own directory, one
//! file per component kind (issue 52). Tools, commands, prompt parts,
//! language models and moderators have directories of their own (`tool`,
//! `command`, `prompt`, `llm`, `moderator`); a
//! component that offers several facets - tools and a prompt part, say -
//! stays here. The facet traits live next to the code that uses them
//! (`crate::tool`, `caelumora_runtime::command`, `crate::prompt`, `crate::message`).

pub mod agent_memory;
pub mod channel_history;
pub mod create_plan_tool;
pub mod notes;
pub mod private_notes;

use caelumora_runtime::component::ComponentRegistry;

/// Registers the component kinds of this directory with `registry`.
pub fn register_all(registry: &mut ComponentRegistry) {
    agent_memory::register(registry);
    channel_history::register(registry);
    create_plan_tool::register(registry);
    notes::register(registry);
    private_notes::register(registry);
}
