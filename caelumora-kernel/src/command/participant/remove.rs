//! The built-in `/participant remove` command (issue 560).

use crate::command::{command_error, named_one, named_participants, taking_part, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const REMOVE_COMMAND_NAME: &str = "participant remove";

/// `/participant remove <participant>`: takes someone out of the channel.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(REMOVE_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Removes the participant named from this channel; they stay in the session."
            .to_string(),
        usage: Some("<participant>".to_string()),
        owners_only: false,
    }
}

/// Runs `/participant remove` with `args`: removes the participant named - among those
/// `/participant list` lists (`taking_part`) and can be named (`named_participants`), found by
/// name or handle (`named_one`), also one that cannot run - from the channel, not from the session
/// (`ChannelLoop::remove_participant`), which asks whether the issuer may change the channel. Only
/// the issuer is answered: the channel tells everyone that someone left.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(CommandError::InvalidArguments {
            command: REMOVE_COMMAND_NAME.to_string(),
            reason: "name whom to remove".to_string(),
        });
    }
    let participants = named_participants(taking_part(REMOVE_COMMAND_NAME, context).await?);
    let (name, participant) =
        named_one(REMOVE_COMMAND_NAME, wanted, participants, |(name, _)| name, "in this channel")?;
    context
        .channel
        .remove_participant(&context.issuer.security_context(), participant.id())
        .await
        .map_err(|e| command_error(REMOVE_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(format!("Removed {name}.")))
}
