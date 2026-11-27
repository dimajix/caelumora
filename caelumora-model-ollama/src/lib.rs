//! Language models served by Ollama (issues 329, 520): the backend `ollama` of the models of
//! `caelumora-model` (`client`), with how a model's control tokens are kept apart from text
//! (`control_tokens`, issues 149, 625), and the plugin that registers it (`plugin`). A crate of its
//! own, so that the kernel names no model provider; which model an agent talks to is the
//! configuration's choice, or the program's.

pub mod client;
mod control_tokens;
pub mod plugin;
