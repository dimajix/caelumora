//! The built-in command group `channel` (issue 560): the commands on the channels of a session -
//! `channel list`, `describe`, `rename`, `summary` and `briefing` -, contributed by one component,
//! one file per command. Those that change a channel change the one they are issued in.

mod briefing;
mod describe;
mod list;
mod rename;
mod summary;

use crate::command::{ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, CHANNEL_GROUP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// The component of the command group `channel`: `/channel list`, `/channel describe`,
/// `/channel rename`, `/channel summary` and `/channel briefing`.
#[derive(Debug)]
pub(crate) struct ChannelGroupComponent;

impl ProvidesCommands for ChannelGroupComponent {
    fn commands(&self) -> Vec<Command> {
        vec![
            list::command(),
            describe::command(),
            rename::command(),
            summary::command(),
            briefing::command(),
        ]
    }
}

#[async_trait]
impl ChannelCommandProvider for ChannelGroupComponent {
    async fn execute(
        &self,
        name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        match name.as_str() {
            list::LIST_COMMAND_NAME => list::execute(args, context).await,
            describe::DESCRIBE_COMMAND_NAME => describe::execute(args, context).await,
            rename::RENAME_COMMAND_NAME => rename::execute(args, context).await,
            summary::SUMMARY_COMMAND_NAME => summary::execute(args, context).await,
            briefing::BRIEFING_COMMAND_NAME => briefing::execute(args, context).await,
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

/// Registers the kind `channel` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_GROUP_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(ChannelGroupComponent));
            Ok(facets)
        },
    );
}
