//! An agent's memory (issues 62, 518): what an agent remembers across its channels and sessions -
//! a system service of its own, as a plugin and a library. The crate is cut into layers by
//! features, as `caelumora-skills` is: without features it is the memory, its changes and the
//! document that keeps an agent's (`memory`); `api` adds what an agent's work reaches it by
//! (`extension`), the crate's part of the kernel's API; `kernel` adds its one keeper (`manager`),
//! how an agent's work reaches it and what may be recalled where (`view`), the question of who may
//! reclassify an entry (`access`) and the plugin that joins all of it to the kernel (`plugin`);
//! `library` adds the `agent_memory` component and the library that registers it (`library`).

pub mod memory;

/// The name of the plugin and of the library.
pub const MEMORY: &str = "memory";

/// The namespace of the texts of the memory in the prompt catalogue (issue 57).
#[cfg(feature = "library")]
pub const NAMESPACE: &str = "memory";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
#[cfg(feature = "library")]
pub const PROMPTS: &str = include_str!("prompts.yaml");

#[cfg(feature = "kernel")]
pub mod access;
#[cfg(feature = "library")]
pub mod component;
#[cfg(feature = "api")]
pub mod extension;
#[cfg(feature = "library")]
pub mod library;
#[cfg(feature = "kernel")]
mod manager;
#[cfg(feature = "kernel")]
pub mod plugin;
#[cfg(feature = "kernel")]
mod view;
