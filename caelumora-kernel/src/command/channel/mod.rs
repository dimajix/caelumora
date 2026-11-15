//! The built-in command group `channel` (issue 560): the commands on the channels of a session -
//! `channel list`, `describe`, `notes`, `create`, `delete`, `join`, `leave`, `rename`, `summary`
//! and `briefing` -, contributed by one component, one file per command. Those that change or
//! leave a channel act on the one they are issued in; those that make, delete or join one name it.

mod briefing;
mod create;
mod delete;
mod describe;
mod join;
mod leave;
mod list;
mod notes;
mod rename;
mod summary;

use crate::command::{command_error, ChannelCommand, CommandContext};
use crate::component::ComponentRegistry;
use crate::view::ChannelView;
use caelumora_api::command::{CommandError, CommandProvider, CommandResult};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::reader::ViewError;
use caelumora_domain::command::CHANNEL_GROUP_KIND;
use std::sync::Arc;

/// The commands of the group `channel`: `/channel list`, `/channel describe`, `/channel notes`,
/// `/channel create`, `/channel delete`, `/channel join`, `/channel leave`, `/channel rename`,
/// `/channel summary` and `/channel briefing`.
fn commands() -> Vec<Arc<dyn ChannelCommand>> {
    vec![
        Arc::new(list::ListCommand::new()),
        Arc::new(describe::DescribeCommand::new()),
        Arc::new(notes::NotesCommand::new()),
        Arc::new(create::CreateCommand::new()),
        Arc::new(delete::DeleteCommand::new()),
        Arc::new(join::JoinCommand::new()),
        Arc::new(leave::LeaveCommand::new()),
        Arc::new(rename::RenameCommand::new()),
        Arc::new(summary::SummaryCommand::new()),
        Arc::new(briefing::BriefingCommand::new()),
    ]
}

/// The channel the command `command` is issued in, or the one of the session `args` names
/// (`channel_named`), as the issuer reads it.
///
/// # Errors
/// As `CommandContext::channel_view` and `channel_named`.
async fn this_or_named(
    command: &str,
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<ChannelView> {
    match args.trim() {
        "" => context.channel_view(command).await,
        wanted => channel_named(command, wanted, context).await,
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
        ComponentKind::new(CHANNEL_GROUP_KIND)
            .provides::<dyn CommandProvider<dyn ChannelCommand>>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(commands()));
            Ok(facets)
        },
    );
}
