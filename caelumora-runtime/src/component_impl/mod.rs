//! The built-in components that belong to no facet's own directory, one
//! file per component kind (issue 52). Tools, commands, language models and
//! moderators have directories of their own (`tool`, `command`,
//! `llm`, `moderator`); a component that offers several facets -
//! tools and a prompt part, say - stays here. The facet traits live next to
//! the code that uses them (`crate::tool`, `crate::command`,
//! `crate::context`, `crate::message`).

use crate::component::ComponentRegistry;

pub mod agent_memory;
pub mod channel_history;
pub mod channel_briefing_prompt_provider;
pub mod channel_language_prompt_provider;
pub mod channel_members_prompt_provider;
pub mod create_plan_tool;
pub mod notes;
pub mod private_notes;
pub mod skills;
pub mod turn_context_provider;
pub mod external_context_provider;
pub mod static_system_prompt_provider;
pub mod tool_sets_prompt_provider;
pub mod workspace_prompt_provider;

/// Registers the component kinds of this directory with `registry`, and the
/// `ContextProvider` as a facet of which an agent has exactly one, with its
/// default kind (issue 99).
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn crate::context::ContextProvider>(Some(turn_context_provider::TURN_CONTEXT_PROVIDER_KIND));

    agent_memory::register(registry);
    channel_history::register(registry);
    channel_briefing_prompt_provider::register(registry);
    channel_language_prompt_provider::register(registry);
    channel_members_prompt_provider::register(registry);
    create_plan_tool::register(registry);
    notes::register(registry);
    private_notes::register(registry);
    skills::register(registry);
    turn_context_provider::register(registry);
    external_context_provider::register(registry);
    static_system_prompt_provider::register(registry);
    tool_sets_prompt_provider::register(registry);
    workspace_prompt_provider::register(registry);
}
