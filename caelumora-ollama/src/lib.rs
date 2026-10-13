//! Language models served by Ollama (issue 329): the component kind
//! `ollama_client`, which provides the runtime's `LLMClient` facet
//! (`client`), and the plugin that registers it (`plugin`). A crate of its
//! own, so that the core keeps the facet alone and names no model provider;
//! which kind is the default model is the program's choice.

pub mod client;
pub mod plugin;
