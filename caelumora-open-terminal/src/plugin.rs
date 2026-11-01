//! How the Open Terminal storages are joined to the kernel (issue 327): as backends of its storages
//! of workspaces alone.

#[cfg(feature = "docker")]
use crate::docker;
use crate::shared;
use anyhow::Result;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;

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

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}
