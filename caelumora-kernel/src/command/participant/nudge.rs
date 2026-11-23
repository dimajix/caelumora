//! The built-in `/participant nudge` command (issues 278, 560), `/nudge` for short.

use crate::channel::ChannelError;
use crate::command::{command_error, named_one, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::value::command_name::CommandName;

const NUDGE_COMMAND_NAME: &str = "participant nudge";

/// `/participant nudge <participant>`: asks someone in the channel to respond to the conversation
/// as it stands, without a message. `/nudge` for short.
#[derive(Debug)]
pub(super) struct NudgeCommand(CommandDescription);

impl NudgeCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(NUDGE_COMMAND_NAME),
            aliases: vec![CommandName::fixed("nudge")],
            description: "Asks the participant named to respond to the conversation as it stands, \
                          without a message."
                .to_string(),
            parameters: vec![
                CommandParameter::required("participant").of_kind(ParameterKind::Participant)
            ],
            owners_only: false,
        })
    }
}

impl Command for NudgeCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for NudgeCommand {
    /// Runs `/participant nudge` with `args`: asks the participant named - an agent, another user -
    /// to respond as an `@`-mention would, with nothing said, in the thread it is issued in
    /// (`UserActor::request_response`, issue 602). Only the issuer is told, and the one asked: the
    /// history keeps nothing of it, and the others see the answer.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: &str| CommandError::InvalidArguments {
            command: NUDGE_COMMAND_NAME.to_string(),
            reason: reason.to_string(),
        };
        let wanted = args.trim();
        // Of those who take part in the thread it is issued in (issue 602), where it asks.
        let there: Vec<_> = context
            .channel
            .participants()
            .into_iter()
            .filter(|m| context.channel.runs_in(m.id, context.thread))
            .collect();
        let participant =
            named_one(NUDGE_COMMAND_NAME, wanted, there, |m| &m.name, "in this thread")?;
        match context.issuer.request_response(participant.id, Some(context.thread)).await {
            Ok(_) => Ok(CommandOutput::private(format!("Asked {} to respond.", participant.name))),
            Err(e) => match e.downcast_ref::<ChannelError>() {
                Some(ChannelError::NudgesItself { .. }) => {
                    Err(invalid("you cannot nudge yourself"))
                }
                _ => Err(command_error(NUDGE_COMMAND_NAME, &e)),
            },
        }
    }
}
