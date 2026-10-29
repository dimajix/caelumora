//! How external agents reached over A2A are joined to the runtime (issue
//! 326): by their component kind alone.

use crate::agent;
use anyhow::Result;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;

/// External agents reached over A2A, as a plugin.
#[derive(Debug, Default)]
pub struct A2aPlugin;

impl Plugin for A2aPlugin {
    fn name(&self) -> &'static str {
        "a2a"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        agent::register(runtime.components());
        Ok(())
    }
}
