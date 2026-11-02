//! How models served by Ollama are joined to the kernel (issues 329, 520): as backends of the
//! models of the plugin `caelumora-models` alone. Which of them an agent talks to the configuration
//! says, and the program where it says none.

use crate::client;
use crate::client::OLLAMA_BACKEND;
use anyhow::Result;
use caelumora_domain::config::{AppConfig, BackendConfig};
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_models::plugin::with_default_model;
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

/// `config`, with a model served by a local Ollama where its section `plugins.models` names none -
/// the backend `ollama` without parameters -, for a program that makes it the model it offers
/// (issues 329, 520).
pub fn with_local_model(config: AppConfig) -> AppConfig {
    with_default_model(config, OLLAMA_BACKEND, BackendConfig::new(OLLAMA_BACKEND, Value::Null))
}
