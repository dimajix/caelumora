//! The built-in language model components, one file per component kind
//! (issue 130). The facet trait, `LLMClient`, lives in `crate::llm`.

use crate::component::ComponentRegistry;

pub mod control_tokens;
pub mod ollama_client;

/// Registers the model kinds with `registry`, and the `LLMClient` as a facet
/// of which an agent has exactly one, with its default kind.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn crate::llm::LLMClient>(ollama_client::OLLAMA_CLIENT_KIND);

    ollama_client::register(registry);
}
