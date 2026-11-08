//! Decision models reached through the contract `POST /v1/systemone` (issue 551), which Ollama
//! serves since version 0.35 and some hosted services implement too: the backend `systemone` of the
//! decision models of `caelumora-model` (`model`) and the plugin that registers it (`plugin`). A
//! crate of its own, named after the contract, not after a provider, so that neither the models'
//! plugin nor what asks a decision model names one.

mod model;
pub mod plugin;
