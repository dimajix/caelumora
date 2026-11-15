//! The built-in `/agent wakeup` command (issues 28, 560), `/wakeup` for short.

use crate::command::{command_error, named_one, names, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::value::command_name::CommandName;

const WAKEUP_COMMAND_NAME: &str = "agent wakeup";

/// `/agent wakeup [agent]`: wakes the sleeping agents of the channel, or one of them. `/wakeup`
/// for short.
#[derive(Debug)]
pub(super) struct WakeupCommand(CommandDescription);

impl WakeupCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(WAKEUP_COMMAND_NAME),
            aliases: vec![CommandName::fixed("wakeup")],
            description: "Wakes the sleeping agents of this channel, or the one named.".to_string(),
            parameters: vec![CommandParameter::optional("agent")],
            owners_only: false,
        })
    }
}

impl Command for WakeupCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for WakeupCommand {
    /// Runs `/agent wakeup` with `args`. Everybody in the channel is told, since it changes who
    /// answers.
    async fn execute(
        &self,
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
        let participant = named_one(
            WAKEUP_COMMAND_NAME,
            wanted,
            context.channel.participants(),
            |m| &m.name,
            "in this channel",
        )?;
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
}
