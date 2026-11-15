//! The built-in command group `participant` (issue 560): the commands on who takes part in a
//! channel - `participant list`, `add`, `remove` and `nudge` -, contributed by one component, one
//! file per command.

mod add;
mod list;
mod nudge;
mod remove;

use crate::command::{ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, PARTICIPANT_GROUP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// The component of the command group `participant`: `/participant list`, `/participant add`
/// (`/add`), `/participant remove` and `/participant nudge` (`/nudge`).
#[derive(Debug)]
pub(crate) struct ParticipantGroupComponent;

impl ProvidesCommands for ParticipantGroupComponent {
    fn commands(&self) -> Vec<Command> {
        vec![list::command(), add::command(), remove::command(), nudge::command()]
    }
}

#[async_trait]
impl ChannelCommandProvider for ParticipantGroupComponent {
    async fn execute(
        &self,
        name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        match name.as_str() {
            list::LIST_COMMAND_NAME => list::execute(args, context).await,
            add::ADD_COMMAND_NAME => add::execute(args, context).await,
            remove::REMOVE_COMMAND_NAME => remove::execute(args, context).await,
            nudge::NUDGE_COMMAND_NAME => nudge::execute(args, context).await,
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

/// Registers the kind `participant` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(PARTICIPANT_GROUP_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(ParticipantGroupComponent));
            Ok(facets)
        },
    );
}
