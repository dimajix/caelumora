//! Language models served by Ollama (issue 329): the component kind `ollama_client`, which provides
//! the kernel's `ModelClient` facet (`client`), with the defusing of a model's control tokens
//! (`control_tokens`, issue 149), and the library that registers it (`library`). A
//! crate of its own, so that the kernel keeps the facet alone and names no model provider; which
//! kind is the default model is the program's choice.

pub mod client;
mod control_tokens;
pub mod library;
