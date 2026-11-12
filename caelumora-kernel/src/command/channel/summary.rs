//! The built-in `/channel summary` command (issue 560).

use crate::command::{command_error, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::text::Summary;

pub(super) const SUMMARY_COMMAND_NAME: &str = "channel summary";

/// `/channel summary [text]`: says in a line what the channel the command is issued in is for.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(SUMMARY_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Says in a line what this channel is for; without a text, removes it."
            .to_string(),
        usage: Some("[text]".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel summary` with `args`: the summary of the channel the command is issued in, its
/// briefing kept (`ChannelLoop::update_description`, which asks `UpdateChannel`); an empty `args`
/// removes it. Its agents are told from their next turn on. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let summary = Some(args.trim()).filter(|text| !text.is_empty());
    if let Some(text) = summary {
        Summary::new(text).map_err(|e| CommandError::InvalidArguments {
            command: SUMMARY_COMMAND_NAME.to_string(),
            reason: e.to_string(),
        })?;
    }
    let channel = context.channel.channel();
    let briefing = channel.briefing.as_ref().map(|briefing| briefing.as_str());
    context
        .channel
        .update_description(&context.issuer.security_context(), summary, briefing)
        .await
        .map_err(|e| command_error(SUMMARY_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(match summary {
        Some(_) => "The summary of this channel is set.",
        None => "The summary of this channel is removed.",
    }))
}
