//! The built-in tool components, one file per component kind (issue 52): the
//! `ToolProvider`s and the default `ToolExecutor`. A component that offers
//! tools along with another facet - a prompt part, say - stays in
//! `component_impl`. The facet traits live in `crate::tool`.

use crate::component::ComponentRegistry;

pub mod calculator_tool;
pub mod default_tool_executor;
pub mod echo_tool;
pub mod mcp_tool;
pub mod sleep_tool;
pub mod stop_tool;

/// Registers the tool kinds with `registry`, and the `ToolExecutor` as a
/// facet of which an agent has exactly one, with its default kind.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn crate::tool::ToolExecutor>(default_tool_executor::DEFAULT_TOOL_EXECUTOR_KIND);

    calculator_tool::register(registry);
    default_tool_executor::register(registry);
    echo_tool::register(registry);
    mcp_tool::register(registry);
    sleep_tool::register(registry);
    stop_tool::register(registry);
}
