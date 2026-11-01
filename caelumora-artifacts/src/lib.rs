//! Artifacts (issues 271, 328): results an agent published, kept as fixed versions beyond the
//! workspace they were copied from, as a plugin of their own and a library (issue 516). The crate
//! is cut into layers by features (cut 11 of issue 323): without features it is the entity
//! (`artifact`) and the wire types (`wire`), which a client reads; `api` adds what an agent's work
//! publishes through and what that fails with (`extension`), the crate's part of the kernel's API;
//! `kernel` adds the one owner of artifacts and their content (`manager`), kept in the storages of
//! content of the kernel (issue 282), how a viewer reaches them, and an agent's work through them
//! (`view`), the questions of who may change, read and add to one (`access`) and the plugin that
//! joins all of it to the kernel (`plugin`); `library` adds the `artifact_tool` component and the
//! library that registers it (`library`); `server` adds the routes.

pub mod artifact;
pub mod wire;

/// The name of the plugin and of the library, and the section of the configuration.
pub const ARTIFACTS: &str = "artifacts";

/// The namespace of the texts of artifacts in the prompt catalogue (issue 57).
#[cfg(feature = "library")]
pub const NAMESPACE: &str = "artifacts";

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
pub mod manager;
#[cfg(feature = "kernel")]
pub mod plugin;
#[cfg(feature = "kernel")]
pub mod view;

#[cfg(feature = "server")]
pub mod routes;
