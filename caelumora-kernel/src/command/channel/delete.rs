//! The built-in `/channel delete` command (issue 560).

use super::channel_named;
use crate::command::{command_error, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::value::command_name::CommandName;

const DELETE_COMMAND_NAME: &str = "channel delete";

/// `/channel delete <channel>`: deletes a channel of the session.
#[derive(Debug)]
pub(super) struct DeleteCommand(CommandDescription);

impl DeleteCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(DELETE_COMMAND_NAME),
            aliases: Vec::new(),
            description:
                "Deletes the channel of this session named, also this one; it can be restored."
                    .to_string(),
            parameters: vec![CommandParameter::required("channel").of_kind(ParameterKind::Channel)],
            owners_only: false,
        })
    }
}

impl Command for DeleteCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for DeleteCommand {
    /// Runs `/channel delete` with `args`: the channel of the session named (`channel_named`) -
    /// also the one the command is issued in - is deleted with its history
    /// (`SessionLoop::delete_channel`, which asks `DeleteChannel` and `UpdateSession`). The command
    /// runs in the issuer's call, not in the channel, so its answer reaches the issuer also when
    /// the channel it is issued in ends. Only the issuer is answered: the session tells everyone
    /// that the channel is gone.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: DELETE_COMMAND_NAME.to_string(),
            reason,
        };
        let wanted = args.trim();
        let channel = channel_named(DELETE_COMMAND_NAME, wanted, context).await?;
        let session = context.session_loop(DELETE_COMMAND_NAME)?;
        let deleted = session
            .delete_channel(&context.issuer.security_context(), channel.id())
            .await
            .map_err(|e| command_error(DELETE_COMMAND_NAME, &e))?;
        if !deleted {
            // Gone since it was found.
            return Err(invalid(format!("no channel '{wanted}' in this session")));
        }
        Ok(CommandOutput::private(format!("Deleted {}; it can be restored.", channel.name())))
    }
}
