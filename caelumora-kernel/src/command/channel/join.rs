//! The built-in `/channel join` command (issue 560).

use crate::channel::ChannelError;
use crate::command::{command_error, CommandContext};
use caelumora_api::actor::Actor;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const JOIN_COMMAND_NAME: &str = "channel join";

/// `/channel join <channel>`: takes the issuer into a channel of the session, by their own act.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(JOIN_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Joins the channel of this session named, if you may join it.".to_string(),
        usage: Some("<channel>".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel join` with `args`: the issuer joins the running channel of the session of that
/// whole name (`SessionLoop::channel_named`, `ChannelLoop::join`, which asks `JoinChannel` in
/// their own context). Who may join a channel reads nothing of it before they did, so it is not
/// sought among those they read; and a channel they may not join - by its access list, or not
/// cleared for it -, or that does not run, is named as one that does not exist, so that channels
/// cannot be probed. Only the issuer is answered: the
/// channel tells everyone that they joined.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: JOIN_COMMAND_NAME.to_string(),
        reason,
    };
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(invalid("name the channel to join".to_string()));
    }
    let nowhere = || invalid(format!("no channel '{wanted}' you may join"));
    let (session, views) = context.session_loop(JOIN_COMMAND_NAME)?;
    let channel = session.channel_named(wanted).ok_or_else(nowhere)?;
    let (issuer, name) = (context.issuer.id(), channel.name());
    if channel.channel().lists(issuer) {
        return Ok(CommandOutput::private(format!("You take part in {name} already.")));
    }
    match channel.join(views.context(), issuer).await {
        Ok(()) => Ok(CommandOutput::private(format!("Joined {name}."))),
        Err(e) => match e.downcast_ref() {
            Some(
                ChannelError::Forbidden { .. }
                | ChannelError::NotCleared { .. }
                | ChannelError::OutOfScope { .. },
            ) => Err(nowhere()),
            _ => Err(command_error(JOIN_COMMAND_NAME, &e)),
        },
    }
}
