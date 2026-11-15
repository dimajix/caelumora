//! The built-in `/channel notes` command (issue 574): what the agents noted in a channel, read as
//! the channel's, as a notice board is read.

use super::this_or_named;
use crate::command::{command_error, CommandContext};
use caelumora_api::command::CommandResult;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const NOTES_COMMAND_NAME: &str = "channel notes";

/// `/channel notes [channel]`: what is noted in a channel.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(NOTES_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Shows what the agents noted in this channel, or in the one of this session \
                      named."
            .to_string(),
        usage: Some("[channel]".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel notes` with `args`: of the channel the command was issued in, or of the one of
/// the session `args` names (`this_or_named`), the notes its agents published
/// (`ChannelView::notes`), by their authors, each note with its number. A user reads a channel
/// whole or not at all, so none is left out. Only the issuer is answered.
///
/// # Errors
/// `CommandError::InvalidArguments` if the session has no channel of that name the issuer may
/// read; `CommandError::Failed` if the session, the channel or an agent's state cannot be read.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let channel = this_or_named(NOTES_COMMAND_NAME, args, context).await?;
    let published =
        channel.notes().await.map_err(|e| command_error(NOTES_COMMAND_NAME, &e.into()))?;
    let mut text = String::new();
    for by in published.iter().filter(|p| !p.notes.is_empty()) {
        text.push_str(&format!("\n{}:\n", by.name));
        for note in &by.notes {
            text.push_str(&format!("- #{} {}\n", note.id, note.text));
        }
    }
    if text.is_empty() {
        return Ok(CommandOutput::private(format!("Nothing is noted in {}.", channel.name())));
    }
    Ok(CommandOutput::private(format!("Noted in {}:\n{}", channel.name(), text.trim_end())))
}
