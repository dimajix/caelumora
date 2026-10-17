//! Decision models (issue 403): models that answer typed questions about a
//! text with probabilities instead of generating text, reached through the
//! `/v1/systemone` contract that Ollama serves since version 0.35 and some
//! hosted services implement too. The crate is named after the contract,
//! not after a provider.
//!
//! It holds the client of the contract (`client`), the component kind
//! `decision_policy`, a `ResponderPolicy` that asks a decision model which
//! agents should respond to a message (`policy`), and the plugin that
//! registers it (`plugin`).

pub mod client;
pub mod plugin;
pub mod policy;
