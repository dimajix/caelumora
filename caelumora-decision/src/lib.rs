//! Choosing by a decision model (issue 403): the component kind `decision_policy`, a
//! `ResponderPolicy` that asks one of the decision models the platform offers which agents should
//! respond to a message (`policy`), and the plugin that registers it (`plugin`). The model itself
//! is the plugin `caelumora-model`'s, and what serves it a backend of that (issue 551).

/// The namespace of the questions of the decision model in the prompt
/// catalogue (issue 57).
pub const NAMESPACE: &str = "decision";

/// Their texts, added to the catalogue under `NAMESPACE` by the plugin.
pub const PROMPTS: &str = include_str!("prompts.yaml");

pub mod plugin;
pub mod policy;
