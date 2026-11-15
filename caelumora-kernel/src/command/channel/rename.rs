//! The built-in `/channel rename` command (issue 560).

use crate::command::{command_error, ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

const RENAME_COMMAND_NAME: &str = "channel rename";

/// `/channel rename <new name>`: renames the channel the command is issued in.
#[derive(Debug)]
pub(super) struct RenameCommand(CommandDescription);

impl RenameCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(RENAME_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Renames this channel.".to_string(),
            parameters: vec![CommandParameter::required("new name")],
            owners_only: false,
        })
    }
}

impl Command for RenameCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for RenameCommand {
    /// Runs `/channel rename` with `args`, the new name of the channel the command is issued in
    /// (`ChannelLoop::update_name`, which asks `UpdateChannel`). Only the issuer is answered: the
    /// channel tells everyone its new name.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: RENAME_COMMAND_NAME.to_string(),
            reason,
        };
        let name = args.trim();
        Name::new(name).map_err(|e| invalid(e.to_string()))?;
        context
            .channel
            .update_name(&context.issuer.security_context(), name)
            .await
            .map_err(|e| command_error(RENAME_COMMAND_NAME, &e))?;
        Ok(CommandOutput::private(format!("This channel is called {name} now.")))
    }
}
