//! The built-in command group `channel` (issue 560): the commands on the channels of a session -
//! `channel list`, `describe`, `create`, `delete`, `join`, `leave`, `rename`, `summary` and
//! `briefing` -, contributed by one component, one file per command. Those that change or leave a
//! channel act on the one they are issued in; those that make, delete or join one name it.

mod briefing;
mod create;
mod delete;
mod describe;
mod join;
mod leave;
mod list;
mod rename;
mod summary;

use crate::command::{command_error, ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use crate::view::ChannelView;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::reader::ViewError;
use caelumora_domain::command::{Command, CommandOutput, CHANNEL_GROUP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// The component of the command group `channel`: `/channel list`, `/channel describe`,
/// `/channel create`, `/channel delete`, `/channel join`, `/channel leave`, `/channel rename`,
/// `/channel summary` and `/channel briefing`.
#[derive(Debug)]
pub(crate) struct ChannelGroupComponent;

impl ProvidesCommands for ChannelGroupComponent {
    fn commands(&self) -> Vec<Command> {
        vec![
            list::command(),
            describe::command(),
            create::command(),
            delete::command(),
            join::command(),
            leave::command(),
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
            create::CREATE_COMMAND_NAME => create::execute(args, context).await,
            delete::DELETE_COMMAND_NAME => delete::execute(args, context).await,
            join::JOIN_COMMAND_NAME => join::execute(args, context).await,
            leave::LEAVE_COMMAND_NAME => leave::execute(args, context).await,
            rename::RENAME_COMMAND_NAME => rename::execute(args, context).await,
            summary::SUMMARY_COMMAND_NAME => summary::execute(args, context).await,
            briefing::BRIEFING_COMMAND_NAME => briefing::execute(args, context).await,
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

/// The channel of the session `wanted` names as the issuer reads it, for the command `command`: by
/// its whole name, which is the only one of the session (`SessionView::channel_named`), a channel
/// they may not read named as one that does not exist.
///
/// # Errors
/// `CommandError::InvalidArguments` if the session has no channel of that name the issuer may
/// read; `CommandError::Failed` if the session cannot be read.
async fn channel_named(
    command: &str,
    wanted: &str,
    context: &CommandContext<'_>,
) -> CommandResult<ChannelView> {
    let session = context.session_view(command).await?;
    match session.channel_named(wanted).await {
        Ok(channel) => Ok(channel),
        Err(e @ ViewError::NoChannelNamed(_)) => Err(CommandError::InvalidArguments {
            command: command.to_string(),
            reason: e.to_string(),
        }),
        Err(e) => Err(command_error(command, &e.into())),
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
