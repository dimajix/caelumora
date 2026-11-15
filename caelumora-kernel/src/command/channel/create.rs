//! The built-in `/channel create` command (issue 560).

use crate::command::{command_error, ChannelCommand, CommandContext};
use crate::session::ChannelSpec;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

const CREATE_COMMAND_NAME: &str = "channel create";

/// `/channel create <name>`: makes a channel in the session, with its issuer in it.
#[derive(Debug)]
pub(super) struct CreateCommand(CommandDescription);

impl CreateCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(CREATE_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Makes a channel of the name given in this session, with you alone in it."
                .to_string(),
            usage: Some("<name>".to_string()),
            owners_only: false,
        })
    }
}

impl Command for CreateCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for CreateCommand {
    /// Runs `/channel create` with `args`, the name of the new channel: made as a channel that says
    /// nothing else (`ChannelSpec::new`), with the issuer alone in it
    /// (`SessionLoop::create_channel`, which asks `CreateChannel` and `UpdateSession`). Only the
    /// issuer is answered: the session tells everyone of the new channel.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: CREATE_COMMAND_NAME.to_string(),
            reason,
        };
        let name = args.trim();
        if name.is_empty() {
            return Err(invalid("name the new channel".to_string()));
        }
        Name::new(name).map_err(|e| invalid(e.to_string()))?;
        let session = context.session_loop(CREATE_COMMAND_NAME)?;
        session
            .create_channel(&context.issuer.security_context(), ChannelSpec::new(name))
            .await
            .map_err(|e| command_error(CREATE_COMMAND_NAME, &e))?;
        Ok(CommandOutput::private(format!("Created the channel {name}; you take part in it.")))
    }
}
