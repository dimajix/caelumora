//! How image models served by ComfyUI are joined to the kernel (issue 525): as backends of the
//! image models of the plugin `caelumora-model` alone.

use crate::model;
use anyhow::Result;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;

/// The backend `comfyui`, as a plugin.
#[derive(Debug, Default)]
pub struct ComfyUiModelPlugin;

impl Plugin for ComfyUiModelPlugin {
    fn name(&self) -> &'static str {
        "model_comfyui"
    }

    fn backends(&self, backends: &mut Backends) {
        model::register_backend(backends);
    }

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}
