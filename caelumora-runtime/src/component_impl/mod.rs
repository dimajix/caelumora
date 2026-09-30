//! The built-in components, one file per component kind (issue 52). A
//! component may offer several facets - a tool provider that also
//! contributes a prompt part, say - so they are sorted by component, not by
//! facet. The facet traits live next to the code that uses them
//! (`crate::tool`, `crate::command`, `crate::context`, `crate::message`).

use crate::component::ComponentRegistry;

pub mod agent_memory;
pub mod calculator_tool;
pub mod channel_history;
pub mod create_plan_tool;
pub mod default_tool_executor;
pub mod echo_tool;
pub mod help_command;
pub mod mcp_tool;
pub mod notes;
pub mod ollama_client;
pub mod private_notes;
pub mod transcript_appender;
pub mod state_context_provider;
pub mod state_history_appender;
pub mod static_system_prompt_provider;
pub mod stop_tool;
pub mod wakeup_command;

/// Registers every built-in component kind with `registry`, and the facets
/// of which an agent has exactly one, with their default kinds (issue 99).
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn crate::context::ContextProvider>(state_context_provider::STATE_CONTEXT_PROVIDER_KIND);
    registry.single::<dyn crate::tool::ToolExecutor>(default_tool_executor::DEFAULT_TOOL_EXECUTOR_KIND);
    registry.single::<dyn crate::llm::LLMClient>(ollama_client::OLLAMA_CLIENT_KIND);

    agent_memory::register(registry);
    calculator_tool::register(registry);
    channel_history::register(registry);
    create_plan_tool::register(registry);
    default_tool_executor::register(registry);
    echo_tool::register(registry);
    help_command::register(registry);
    mcp_tool::register(registry);
    notes::register(registry);
    ollama_client::register(registry);
    private_notes::register(registry);
    transcript_appender::register(registry);
    state_context_provider::register(registry);
    state_history_appender::register(registry);
    static_system_prompt_provider::register(registry);
    stop_tool::register(registry);
    wakeup_command::register(registry);
}
