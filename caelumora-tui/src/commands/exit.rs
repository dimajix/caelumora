//! `/exit` (issue 570): the program ended.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;

/// `/exit` (`/quit`): ends the program. Of no group, as `/help`: ending the program acts on no
/// entity of a channel.
#[derive(Debug)]
pub(super) struct ExitCommand(CommandDescription);

impl ExitCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("exit"),
            aliases: vec![CommandName::fixed("quit")],
            description: "Ends the program, the session shut down in order.".to_string(),
            usage: None,
            owners_only: false,
        })
    }
}

impl Command for ExitCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for ExitCommand {
    async fn execute(&self, _args: &str, _talk: &mut Talk) -> Handled {
        Handled::Exit
    }
}
