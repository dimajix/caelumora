//! The built-in `/channel delete` command (issue 560).

use super::channel_named;
use crate::command::{command_error, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const DELETE_COMMAND_NAME: &str = "channel delete";

/// `/channel delete <channel>`: deletes a channel of the session.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(DELETE_COMMAND_NAME),
        aliases: Vec::new(),
        description:
            "Deletes the channel of this session named, also this one; it can be restored."
                .to_string(),
        usage: Some("<channel>".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel delete` with `args`: the channel of the session named (`channel_named`) - also
/// the one the command is issued in - is deleted with its history (`SessionLoop::delete_channel`,
/// which asks `DeleteChannel` and `UpdateSession`). The command runs in the issuer's call, not in
/// the channel, so its answer reaches the issuer also when the channel it is issued in ends. Only
/// the issuer is answered: the session tells everyone that the channel is gone.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: DELETE_COMMAND_NAME.to_string(),
        reason,
    };
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(invalid("name the channel to delete".to_string()));
    }
    let channel = channel_named(DELETE_COMMAND_NAME, wanted, context).await?;
    let (session, views) = context.session_loop(DELETE_COMMAND_NAME)?;
    let deleted = session
        .delete_channel(views.context(), channel.id())
        .await
        .map_err(|e| command_error(DELETE_COMMAND_NAME, &e))?;
    if !deleted {
        // Gone since it was found.
        return Err(invalid(format!("no channel '{wanted}' in this session")));
    }
    Ok(CommandOutput::private(format!("Deleted {}; it can be restored.", channel.name())))
}
