//! Skills (issues 22, 323): procedures written down for agents to follow,
//! as a plugin of their own - the first subject moved out of the core. The
//! crate is cut into layers by features (cut 11 of issue 323): without
//! features it is the entity (`skill`) and the wire types (`api`), which a
//! client reads; `runtime` adds the one writer of skills (`manager`), how
//! a viewer reaches them (`view`), the questions of who may use and change
//! one (`access`), the `skills` component and the plugin that joins all of
//! it to the runtime (`plugin`); `server` adds the routes.

pub mod api;
pub mod skill;

/// The namespace of the texts of skills in the prompt catalogue (issue 57).
#[cfg(feature = "runtime")]
pub const NAMESPACE: &str = "skills";

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
