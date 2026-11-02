//! Language models served by Ollama (issues 329, 520): the backend `ollama` of the models of
//! `caelumora-models` (`client`), with the defusing of a model's control tokens (`control_tokens`,
//! issue 149), and the plugin that registers it (`plugin`). A crate of its own, so that the kernel
//! names no model provider; which model an agent talks to is the configuration's choice, or the
//! program's.

pub mod client;
mod control_tokens;
pub mod plugin;
