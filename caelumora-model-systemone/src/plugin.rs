//! How decision models reached through `/v1/systemone` are joined to the kernel (issue 551): as
//! backends of the decision models of the plugin `caelumora-model` alone.

use crate::model;
use anyhow::Result;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;

/// The backend `systemone`, as a plugin.
#[derive(Debug, Default)]
pub struct SystemOneModelPlugin;

impl Plugin for SystemOneModelPlugin {
    fn name(&self) -> &'static str {
        "model_systemone"
    }

    fn backends(&self, backends: &mut Backends) {
        model::register_backend(backends);
    }

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}
