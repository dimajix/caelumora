//! What an agent is given to work on the session's workspace (issue 428):
//! the component kinds `terminal_tool` (`terminal_tool`), with the tools on
//! its files (`files`) and those of its shell (`shell`), and
//! `workspace_prompt_provider`, which tells the agent which workspace it
//! works on (`workspace_prompt_provider`), and the plugin that registers
//! both (`plugin`). Workspaces themselves are the core's
//! (issue 333); these are built on the terminal's public API and the
//! agents' tools and prompt parts, as the tools of any other subject are.

pub mod plugin;
pub mod terminal_tool;
pub mod workspace_prompt_provider;

mod files;
mod shell;

#[cfg(test)]
use caelumora_agents::testing;
#[cfg(test)]
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptTexts};
#[cfg(test)]
use std::sync::Arc;

/// The namespace of the texts of the tools of the workspace in the prompt
/// catalogue (issue 57).
pub const NAMESPACE: &str = "workspace_tools";

/// Their texts, added to the catalogue under `NAMESPACE` by the plugin.
pub const PROMPTS: &str = include_str!("prompts.yaml");

/// The prompt catalogue with the texts of the core, of the agents and of
/// these tools, as the runtime and the plugins add them - for what a test
/// makes itself.
#[cfg(test)]
pub(crate) fn prompts() -> PromptTexts {
    PromptTexts::new(catalogue(), None)
}

/// The prompt catalogue with the texts of the core, of the agents and of
/// these tools, as the runtime and the plugins add them.
#[cfg(test)]
pub(crate) fn catalogue() -> Arc<PromptCatalogue> {
    testing::catalogue_with(NAMESPACE, PROMPTS)
}
