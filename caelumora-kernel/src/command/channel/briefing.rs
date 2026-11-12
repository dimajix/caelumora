//! The built-in `/channel briefing` command (issue 560).

use crate::command::{command_error, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::text::Briefing;

pub(super) const BRIEFING_COMMAND_NAME: &str = "channel briefing";

/// `/channel briefing [text]`: says what an agent should know before it works in the channel the
/// command is issued in.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(BRIEFING_COMMAND_NAME),
        aliases: Vec::new(),
        description:
            "Says what an agent should know before it works in this channel; without a text, removes it."
                .to_string(),
        usage: Some("[text]".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel briefing` with `args`: the briefing of the channel the command is issued in, its
/// summary kept (`ChannelLoop::update_description`, which asks `UpdateChannel`); an empty `args`
/// removes it. Its agents are told from their next turn on. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let briefing = Some(args.trim()).filter(|text| !text.is_empty());
    if let Some(text) = briefing {
        Briefing::new(text).map_err(|e| CommandError::InvalidArguments {
            command: BRIEFING_COMMAND_NAME.to_string(),
            reason: e.to_string(),
        })?;
    }
    let channel = context.channel.channel();
    let summary = channel.summary.as_ref().map(|summary| summary.as_str());
    context
        .channel
        .update_description(&context.issuer.security_context(), summary, briefing)
        .await
        .map_err(|e| command_error(BRIEFING_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(match briefing {
        Some(_) => "The briefing of this channel is set.",
        None => "The briefing of this channel is removed.",
    }))
}
