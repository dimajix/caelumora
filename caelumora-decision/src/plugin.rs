//! How decision models are joined to the runtime (issue 403): by the
//! component kind `decision_policy`.

use crate::policy;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use anyhow::Result;

/// Decision models, as a plugin.
#[derive(Debug, Default)]
pub struct DecisionPlugin;

impl Plugin for DecisionPlugin {
    fn name(&self) -> &'static str {
        "decision"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        policy::register(runtime.components());
        Ok(())
    }
}
