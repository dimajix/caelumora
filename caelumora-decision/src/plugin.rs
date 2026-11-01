//! How decision models are joined to the kernel (issue 403): by the component kind
//! `decision_policy`.

use crate::policy;
use crate::{NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;

/// Decision models, as a plugin.
#[derive(Debug, Default)]
pub struct DecisionPlugin;

impl Plugin for DecisionPlugin {
    fn name(&self) -> &'static str {
        "decision"
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        kernel.prompts().add(NAMESPACE, PROMPTS)?;
        policy::register(kernel.components());
        Ok(())
    }
}
