//! The built-in `/channel list` command (issue 560).

use crate::command::CommandContext;
use caelumora_api::actor::Actor;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const LIST_COMMAND_NAME: &str = "channel list";

/// `/channel list`: the channels of the session.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(LIST_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Lists the channels of this session you may read.".to_string(),
        usage: None,
        owners_only: false,
    }
}

/// Runs `/channel list`: the channels of the session the issuer may read as a whole
/// (`SessionView::channels`), in the order of the session, a line each, with whether it is the one
/// the command was issued in, whether the issuer takes part, and its summary. Only the issuer is
/// answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    if !args.trim().is_empty() {
        return Err(CommandError::InvalidArguments {
            command: LIST_COMMAND_NAME.to_string(),
            reason: "it takes no arguments".to_string(),
        });
    }
    let session = context.session_view(LIST_COMMAND_NAME).await?;
    let issuer = context.issuer.id();
    let mut text = String::from("The channels of this session:\n\n");
    for channel in session.channels().await {
        let here = if channel.id() == context.channel.id() { " (here)" } else { "" };
        let part = if channel.lists(issuer) { "taking part" } else { "not taking part" };
        let summary = channel.summary().map(|s| format!(" - {}", s.as_str())).unwrap_or_default();
        text.push_str(&format!("- {}{here}, {part}{summary}\n", channel.name()));
    }
    Ok(CommandOutput::private(text))
}
