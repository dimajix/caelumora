//! The built-in `/agent cancel` command (issue 611), `/cancel` for short.

use crate::command::{command_error, named_one, names, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::channel::ThreadId;
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::value::command_name::CommandName;

const CANCEL_COMMAND_NAME: &str = "agent cancel";

/// `/agent cancel [agent]`: ends the turns of the agents of the channel - of the thread it is
/// issued in, where that is not the line -, or the turn of one of them, leaving them in the
/// channel. `/cancel` for short.
#[derive(Debug)]
pub(super) struct CancelCommand(CommandDescription);

impl CancelCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(CANCEL_COMMAND_NAME),
            aliases: vec![CommandName::fixed("cancel")],
            description: "Ends the turns of the agents of this channel - of this thread, issued in one - or the turn of the one named.".to_string(),
            parameters: vec![CommandParameter::optional("agent").of_kind(ParameterKind::Agent)],
            owners_only: false,
        })
    }
}

impl Command for CancelCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for CancelCommand {
    /// Runs `/agent cancel` with `args`: without an agent, as `ChannelLoop::cancel`, which keeps
    /// what it ended quiet until someone speaks; with one, as `ChannelLoop::cancel_actor`. Both ask
    /// for `CancelChannel`. Everybody in the channel is told, since it changes who answers.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        if wanted.is_empty() {
            // Issued in a thread, it ends the turns of that thread; in the line, of the channel.
            let line = ThreadId::line(context.channel.id());
            let thread = Some(context.thread).filter(|thread| *thread != line);
            let mut ended = context
                .channel
                .cancel(&context.issuer.security_context(), thread)
                .await
                .map_err(|e| command_error(CANCEL_COMMAND_NAME, &e))?;
            ended.sort();
            let place = if thread.is_some() { "thread" } else { "channel" };
            let text = if ended.is_empty() {
                format!("No agent had a turn; this {place} is quiet until someone speaks.")
            } else {
                format!(
                    "Ended the turn of {}; this {place} is quiet until someone speaks.",
                    names(&ended)
                )
            };
            return Ok(CommandOutput::channel(text));
        }

        // Anyone taking part, as `/agent wakeup` takes them; only an agent has a turn.
        let participant = named_one(
            CANCEL_COMMAND_NAME,
            wanted,
            context.channel.participants(),
            |m| &m.name,
            "in this channel",
        )?;
        let had_a_turn = context
            .channel
            .cancel_actor(&context.issuer.security_context(), participant.id)
            .await
            .map_err(|e| command_error(CANCEL_COMMAND_NAME, &e))?;
        let text = if had_a_turn {
            format!("Ended the turn of {}.", participant.name)
        } else {
            format!("{} had no turn.", participant.name)
        };
        Ok(CommandOutput::channel(text))
    }
}
