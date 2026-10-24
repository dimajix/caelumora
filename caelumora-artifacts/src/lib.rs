//! Artifacts (issues 271, 328): results an agent published, kept as fixed
//! versions beyond the workspace they were copied from, as a plugin of
//! their own. The crate is cut into layers by features (cut 11 of issue
//! 323): without features it is the entity (`artifact`) and the wire types
//! (`api`), which a client reads; `runtime` adds the one owner of artifacts
//! and their content (`manager`), kept in the storages of content of the
//! core (issue 282), how a viewer reaches them
//! (`view`), the questions of who may change, read and add to one
//! (`access`), the `artifact_tool` component and the plugin that joins all
//! of it to the runtime (`plugin`); `server` adds the routes.

pub mod api;
pub mod artifact;

/// The namespace of the texts of artifacts in the prompt catalogue (issue
/// 57).
#[cfg(feature = "runtime")]
pub const NAMESPACE: &str = "artifacts";

/// Their texts, added to the catalogue under `NAMESPACE` by the plugin.
#[cfg(feature = "runtime")]
pub const PROMPTS: &str = include_str!("prompts.yaml");

#[cfg(feature = "runtime")]
pub mod access;
#[cfg(feature = "runtime")]
pub mod component;
#[cfg(feature = "runtime")]
pub mod manager;
#[cfg(feature = "runtime")]
pub mod plugin;
#[cfg(feature = "runtime")]
pub mod view;

#[cfg(feature = "server")]
pub mod routes;
