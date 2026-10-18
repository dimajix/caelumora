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
