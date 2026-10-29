//! How models served by Ollama are joined to the runtime (issue 329): by
//! their component kind alone. Whether it is the default model the program
//! says, not the plugin (`ComponentRegistry::default_kind`).

use crate::client;
use anyhow::Result;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;

/// Models served by Ollama, as a plugin.
#[derive(Debug, Default)]
pub struct OllamaPlugin;

impl Plugin for OllamaPlugin {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        client::register(runtime.components());
        Ok(())
    }
}
