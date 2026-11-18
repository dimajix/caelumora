//! Sub-agents (issue 558): an agent hands a task to another agent, made for it, which works on it
//! apart, in a thread of its own, and gives back only what came of it - as a plugin and a library
//! (issue 516), cut into layers by features as artifacts, memory and skills are: `api` is what an
//! agent's work does with sub-agents and what that fails with (`extension`), the crate's part of
//! the kernel's API; `kernel` adds the writer of it, made of the kernel's building blocks of
//! threads - opening one, delegating into it, closing it (issue 557) - (`writer`), the names it
//! gives them (`names`) and the plugin that binds it (`plugin`); `library` adds the `subagents`
//! component, its tools, and the library that registers it (`library`).

/// The name of the plugin and of the library.
pub const SUBAGENTS: &str = "subagents";

/// The namespace of the texts of sub-agents in the prompt catalogue (issue 57).
#[cfg(feature = "library")]
pub const NAMESPACE: &str = "subagents";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
#[cfg(feature = "library")]
pub const PROMPTS: &str = include_str!("prompts.yaml");

#[cfg(feature = "library")]
pub mod component;
#[cfg(feature = "api")]
pub mod extension;
#[cfg(feature = "library")]
pub mod library;
#[cfg(feature = "kernel")]
mod names;
#[cfg(feature = "kernel")]
pub mod plugin;
#[cfg(feature = "kernel")]
mod writer;
