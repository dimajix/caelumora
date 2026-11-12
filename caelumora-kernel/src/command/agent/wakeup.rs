//! The built-in `/agent wakeup` command (issues 28, 560), `/wakeup` for short.

use crate::command::{command_error, named_participant, names, CommandContext};
use caelumora_api::command::CommandResult;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const WAKEUP_COMMAND_NAME: &str = "agent wakeup";

/// `/agent wakeup [agent]`: wakes the sleeping agents of the channel, or one of them. `/wakeup`
/// for short.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(WAKEUP_COMMAND_NAME),
        aliases: vec![CommandName::fixed("wakeup")],
        description: "Wakes the sleeping agents of this channel, or the one named.".to_string(),
        usage: Some("[agent]".to_string()),
        owners_only: false,
    }
}

/// Runs `/agent wakeup` with `args`. Everybody in the channel is told, since it changes who
/// answers.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let wanted = args.trim();
    if wanted.is_empty() {
        let mut woken = context
            .channel
            .wakeup(&context.issuer.security_context())
            .await
            .map_err(|e| command_error(WAKEUP_COMMAND_NAME, &e))?;
        woken.sort();
        let text = if woken.is_empty() {
            "No agent was asleep.".to_string()
        } else {
            format!("Woke {}.", names(&woken))
        };
        return Ok(CommandOutput::channel(text));
    }

    // Anyone taking part, as `/participant nudge` takes them; only an agent sleeps.
    let participant = named_participant(WAKEUP_COMMAND_NAME, wanted, context.channel)?;
    let was_asleep = context
        .channel
        .wakeup_actor(&context.issuer.security_context(), participant.id)
        .await
        .map_err(|e| command_error(WAKEUP_COMMAND_NAME, &e))?;
    let text = if was_asleep {
        format!("Woke {}.", participant.name)
    } else {
        format!("{} was not asleep.", participant.name)
    };
    Ok(CommandOutput::channel(text))
}
