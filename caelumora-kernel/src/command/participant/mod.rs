//! The built-in command group `participant` (issue 560): the commands on who takes part in a
//! channel - `participant list`, `add`, `remove` and `nudge` -, contributed by one component, one
//! file per command.

mod add;
mod list;
mod nudge;
mod remove;

use crate::command::ChannelCommand;
use crate::component::ComponentRegistry;
use caelumora_api::command::CommandProvider;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::PARTICIPANT_GROUP_KIND;
use std::sync::Arc;

/// The commands of the group `participant`: `/participant list`, `/participant add` (`/add`),
/// `/participant remove` and `/participant nudge` (`/nudge`).
pub(crate) fn commands() -> Vec<Arc<dyn ChannelCommand>> {
    vec![
        Arc::new(list::ListCommand::new()),
        Arc::new(add::AddCommand::new()),
        Arc::new(remove::RemoveCommand::new()),
        Arc::new(nudge::NudgeCommand::new()),
    ]
}

/// Registers the kind `participant` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(PARTICIPANT_GROUP_KIND)
            .provides::<dyn CommandProvider<dyn ChannelCommand>>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(commands()));
            Ok(facets)
        },
    );
}
