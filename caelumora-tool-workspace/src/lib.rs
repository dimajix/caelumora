//! What an agent is given to work on the session's workspace (issues 428, 532): the component kind
//! `workspace` (`component`), with the tools on its files and those of its shell, and the prompt
//! part that tells the agent which workspace it works on, and the library that registers it
//! (`library`). Workspaces themselves are the kernel's (issue 333); this is built on the terminal's
//! public API and the agents' tools and prompt parts, as the tools of any other subject are.

pub mod component;
pub mod library;

#[cfg(test)]
use caelumora_agents::testing;
#[cfg(test)]
use caelumora_domain::prompt_catalogue::{PromptCatalogue, PromptTexts};
#[cfg(test)]
use std::sync::Arc;

/// The namespace of the texts of the tools of the workspace in the prompt
/// catalogue (issue 57).
pub(crate) const NAMESPACE: &str = "tool_workspace";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
pub(crate) const PROMPTS: &str = include_str!("prompts.yaml");

/// The prompt catalogue with the texts of the kernel, of the agents and of these tools, as the
/// kernel and the libraries add them - for what a test makes itself.
#[cfg(test)]
pub(crate) fn prompts() -> PromptTexts {
    PromptTexts::new(catalogue(), None)
}

/// The prompt catalogue with the texts of the kernel, of the agents and of these tools, as the
/// kernel and the libraries add them.
#[cfg(test)]
pub(crate) fn catalogue() -> Arc<PromptCatalogue> {
    testing::catalogue_with(NAMESPACE, PROMPTS)
}
