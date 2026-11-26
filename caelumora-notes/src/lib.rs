//! An agent's notes in a channel (issues 17, 51, 607): what it publishes there and what it keeps
//! there for itself - a system service of its own, as a plugin and a library. The crate is cut
//! into layers by features, as `caelumora-memory` is: without features it is the notes, their
//! changes, the document that keeps an agent's in a channel (`note`) and their wire (`wire`); `api`
//! adds what an agent's work reaches them by (`extension`), the crate's part of the kernel's API;
//! `kernel` adds their one keeper (`manager`), how an agent's work and the frontends reach them
//! (`view`), the command `/channel notes` (`command`) and the plugin that joins all of it to the
//! kernel (`plugin`); `server` adds the routes (`routes`); `library` adds the components `notes`
//! and `private_notes` and the library that registers them (`library`).

pub mod note;
pub mod wire;

/// The name of the plugin and of the library.
pub const NOTES: &str = "notes";

/// The channel's announcement that the notes of one of its agents changed, without what changed:
/// whoever shows its board reads it again.
pub const NOTES_CHANGED: &str = "notes_changed";

/// The namespace of the texts of the notes in the prompt catalogue (issue 57).
#[cfg(feature = "library")]
pub const NAMESPACE: &str = "notes";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
#[cfg(feature = "library")]
pub const PROMPTS: &str = include_str!("prompts.yaml");

#[cfg(feature = "kernel")]
pub mod command;
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
#[cfg(feature = "server")]
mod routes;
#[cfg(feature = "kernel")]
pub mod view;
