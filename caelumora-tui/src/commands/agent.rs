//! The terminal's command on the agents (issue 570): `/agent retry`.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;

/// `/agent retry` (`/retry`): asks an agent for its answer again.
#[derive(Debug)]
pub(super) struct RetryCommand(CommandDescription);

impl RetryCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("agent retry"),
            aliases: vec![CommandName::fixed("retry")],
            description: "Asks for the answer under way, the turn given up, or the last answer of an agent again.".to_string(),
            parameters: Vec::new(),
            owners_only: false,
        })
    }
}

impl Command for RetryCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for RetryCommand {
    async fn execute(&self, _args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.retry().await)
    }
}
