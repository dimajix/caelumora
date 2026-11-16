//! The command group `workspace` (issue 587): the commands a person runs in the session's
//! workspace - `workspace run` -, contributed by one component of the kind `shell`, one file per
//! command; `workspace` is the kind of the agents' tools on it. No deployment has it by default
//! (`default_commands`): a command run so acts wherever the workspace's storage reaches, so the
//! program of one person on their own machine adds it, the terminal.

mod run;

pub use run::RUN_COMMAND_NAME;

use crate::command::ChannelCommand;
use crate::component::ComponentRegistry;
use caelumora_api::command::CommandProvider;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use std::sync::Arc;

/// Component kind of the command group `workspace`: what it gives a person, a shell.
pub const SHELL_KIND: &str = "shell";

/// The commands of the group `workspace`: `/workspace run`.
fn commands() -> Vec<Arc<dyn ChannelCommand>> {
    vec![Arc::new(run::RunCommand::new())]
}

/// Registers the kind `shell` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(SHELL_KIND).provides::<dyn CommandProvider<dyn ChannelCommand>>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(commands()));
            Ok(facets)
        },
    );
}
