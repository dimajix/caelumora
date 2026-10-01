//! The built-in command components, one file per command (issue 28). The
//! facet trait, `CommandProvider`, lives in `crate::command`.

use crate::component::ComponentRegistry;

pub mod help_command;
pub mod wakeup_command;

/// Registers the command kinds with `registry`.
pub fn register_all(registry: &mut ComponentRegistry) {
    help_command::register(registry);
    wakeup_command::register(registry);
}
