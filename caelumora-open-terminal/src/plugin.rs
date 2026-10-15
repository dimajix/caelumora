//! How the Open Terminal storages are joined to the runtime (issue 327): as
//! backends of its storages of workspaces alone.

use crate::shared;
#[cfg(feature = "docker")]
use crate::docker;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use anyhow::Result;

/// The storages `open_terminal_shared` and, with the feature `docker`,
/// `open_terminal_docker`, as a plugin.
#[derive(Debug, Default)]
pub struct OpenTerminalPlugin;

impl Plugin for OpenTerminalPlugin {
    fn name(&self) -> &'static str {
        "open_terminal"
    }

    fn backends(&self, backends: &mut Backends) {
        shared::register_backend(backends);
        #[cfg(feature = "docker")]
        docker::register_backend(backends);
    }

    fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
        Ok(())
    }
}
