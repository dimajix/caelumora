//! The terminal's command on the channels of the session (issue 570): `/channel switch`.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::{CommandDescription, CommandParameter, ParameterKind};
use caelumora_domain::value::command_name::CommandName;

/// `/channel switch [channel]` (`/switch`): talks in another channel the user takes part in;
/// without one, tells where the user could.
#[derive(Debug)]
pub(super) struct SwitchCommand(CommandDescription);

impl SwitchCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("channel switch"),
            aliases: vec![CommandName::fixed("switch")],
            description: "Talks in another channel of the session you take part in.".to_string(),
            parameters: vec![CommandParameter::optional("channel").of_kind(ParameterKind::Channel)],
            owners_only: false,
        })
    }
}

impl Command for SwitchCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for SwitchCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.switch_channel(args).await)
    }
}
