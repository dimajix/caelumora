//! The built-in `/participant remove` command (issue 560).

use super::taking_part;
use crate::channel::ChannelParticipant;
use crate::command::{command_error, named_participant, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

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
/// `/participant list` lists (`taking_part`), found by name or handle (`named_participant`), also
/// one that cannot run - from the channel, not from the session
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
    // One that no longer exists has no name to be named by.
    let participants = taking_part(REMOVE_COMMAND_NAME, context)
        .await?
        .into_iter()
        .filter_map(|p| {
            let name = Name::new(p.name()?).ok()?;
            Some(ChannelParticipant { id: p.id(), name, handle: p.handle() })
        })
        .collect();
    let participant = named_participant(REMOVE_COMMAND_NAME, wanted, participants)?;
    context
        .channel
        .remove_participant(&context.issuer.security_context(), participant.id)
        .await
        .map_err(|e| command_error(REMOVE_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(format!("Removed {}.", participant.name)))
}
