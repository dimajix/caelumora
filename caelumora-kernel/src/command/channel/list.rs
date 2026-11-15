//! The built-in `/channel list` command (issue 560).

use crate::command::{ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::actor::Actor;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

const LIST_COMMAND_NAME: &str = "channel list";

/// `/channel list`: the channels of the session.
#[derive(Debug)]
pub(super) struct ListCommand(CommandDescription);

impl ListCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(LIST_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Lists the channels of this session you may read.".to_string(),
            usage: None,
            owners_only: false,
        })
    }
}

impl Command for ListCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for ListCommand {
    /// Runs `/channel list`: the channels of the session the issuer may read as a whole
    /// (`SessionView::channels`), in the order of the session, a line each, with whether it is the
    /// one the command was issued in, whether the issuer takes part, and its summary. Only the
    /// issuer is answered.
    async fn execute(
        &self,
        _args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let session = context.session_view(LIST_COMMAND_NAME).await?;
        let issuer = context.issuer.id();
        let mut text = String::from("The channels of this session:\n\n");
        for channel in session.channels().await {
            let here = if channel.id() == context.channel.id() { " (here)" } else { "" };
            let part = if channel.lists(issuer) { "taking part" } else { "not taking part" };
            let summary =
                channel.summary().map(|s| format!(" - {}", s.as_str())).unwrap_or_default();
            text.push_str(&format!("- {}{here}, {part}{summary}\n", channel.name()));
        }
        Ok(CommandOutput::private(text))
    }
}
