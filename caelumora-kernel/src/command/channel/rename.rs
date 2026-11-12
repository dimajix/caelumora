//! The built-in `/channel rename` command (issue 560).

use crate::command::{command_error, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

pub(super) const RENAME_COMMAND_NAME: &str = "channel rename";

/// `/channel rename <new name>`: renames the channel the command is issued in.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(RENAME_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Renames this channel.".to_string(),
        usage: Some("<new name>".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel rename` with `args`, the new name of the channel the command is issued in
/// (`ChannelLoop::update_name`, which asks `UpdateChannel`). Only the issuer is answered: the
/// channel tells everyone its new name.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: RENAME_COMMAND_NAME.to_string(),
        reason,
    };
    let name = args.trim();
    if name.is_empty() {
        return Err(invalid("name the channel's new name".to_string()));
    }
    Name::new(name).map_err(|e| invalid(e.to_string()))?;
    context
        .channel
        .update_name(&context.issuer.security_context(), name)
        .await
        .map_err(|e| command_error(RENAME_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(format!("This channel is called {name} now.")))
}
