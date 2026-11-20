//! The built-in `/channel briefing` command (issue 560).

use crate::command::{command_error, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::text::Briefing;

const BRIEFING_COMMAND_NAME: &str = "channel briefing";

/// `/channel briefing [text]`: says what an agent should know before it works in the channel the
/// command is issued in.
#[derive(Debug)]
pub(super) struct BriefingCommand(CommandDescription);

impl BriefingCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(BRIEFING_COMMAND_NAME),
            aliases: Vec::new(),
            description:
                "Says what an agent should know before it works in this channel; without a text, removes it."
                    .to_string(),
            parameters: vec![CommandParameter::optional("text")],
            owners_only: false,
        })
    }
}

impl Command for BriefingCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for BriefingCommand {
    /// Runs `/channel briefing` with `args`: the briefing of the channel the command is issued in,
    /// its summary kept (`ChannelController::update_description`, which asks `UpdateChannel`); an
    /// empty `args` removes it. Its agents are told from their next turn on. Only the issuer is
    /// answered.
    async fn execute(
        &self,
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
            .controller()
            .update_description(&context.issuer.security_context(), summary, briefing)
            .await
            .map_err(|e| command_error(BRIEFING_COMMAND_NAME, &e.into()))?;
        Ok(CommandOutput::private(match briefing {
            Some(_) => "The briefing of this channel is set.",
            None => "The briefing of this channel is removed.",
        }))
    }
}
