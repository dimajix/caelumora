//! The built-in `/participant nudge` command (issues 278, 560), `/nudge` for short.

use crate::channel::ChannelError;
use crate::command::{command_error, named_participant, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const NUDGE_COMMAND_NAME: &str = "participant nudge";

/// `/participant nudge <participant>`: asks someone in the channel to respond to the conversation
/// as it stands, without a message. `/nudge` for short.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(NUDGE_COMMAND_NAME),
        aliases: vec![CommandName::fixed("nudge")],
        description: "Asks the participant named to respond to the conversation as it stands, without a message.".to_string(),
        usage: Some("<participant>".to_string()),
        owners_only: false,
    }
}

/// Runs `/participant nudge` with `args`: asks the participant named - an agent, another user - to
/// respond as an `@`-mention would, with nothing said (`UserActor::request_response`). Only the
/// issuer is told, and the one asked: the history keeps nothing of it, and the others see the
/// answer.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: &str| CommandError::InvalidArguments {
        command: NUDGE_COMMAND_NAME.to_string(),
        reason: reason.to_string(),
    };
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(invalid("name who is to respond"));
    }
    let participant = named_participant(NUDGE_COMMAND_NAME, wanted, context.channel)?;
    match context.issuer.request_response(participant.id).await {
        Ok(_) => Ok(CommandOutput::private(format!("Asked {} to respond.", participant.name))),
        Err(e) => match e.downcast_ref::<ChannelError>() {
            Some(ChannelError::AlreadyResponding { .. }) => {
                Ok(CommandOutput::private(format!("{} is responding already.", participant.name)))
            }
            Some(ChannelError::NudgesItself { .. }) => Err(invalid("you cannot nudge yourself")),
            _ => Err(command_error(NUDGE_COMMAND_NAME, &e)),
        },
    }
}
