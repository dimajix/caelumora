//! How models served by Ollama are joined to the kernel (issues 329, 520): as backends of its
//! models alone. Which of them an agent talks to the configuration says, and the program where it
//! says none.

use crate::client;
use crate::client::OLLAMA_BACKEND;
use anyhow::Result;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_model::config::{AppConfig, BackendConfig};
use serde_json::Value;

/// The backend `ollama`, as a plugin.
#[derive(Debug, Default)]
pub struct OllamaPlugin;

impl Plugin for OllamaPlugin {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn backends(&self, backends: &mut Backends) {
        client::register_backend(backends);
    }

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}

/// `config`, with a model served by a local Ollama where it names none - the backend `ollama`
/// without parameters -, for a program that makes it the model it names (issues 329, 520).
pub fn with_local_model(mut config: AppConfig) -> AppConfig {
    if config.models.is_empty() {
        let model = BackendConfig::new(OLLAMA_BACKEND, Value::Null);
        config.models.insert(OLLAMA_BACKEND.to_string(), model);
    }
    config
}
