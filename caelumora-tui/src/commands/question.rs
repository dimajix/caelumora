//! The terminal's commands on the questions put to the user (issues 570, 571): `/question answer`
//! and `decline`.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;

/// `/question answer [id]` (`/answer`): asks questions put to the user, those named or those put
/// aside.
#[derive(Debug)]
pub(super) struct AnswerCommand(CommandDescription);

impl AnswerCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("question answer"),
            aliases: vec![CommandName::fixed("answer")],
            description:
                "Asks the questions put to you whose id begins so, or those put aside, one by one."
                    .to_string(),
            usage: Some("[id]".to_string()),
            owners_only: false,
        })
    }
}

impl Command for AnswerCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for AnswerCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.begin_answering(args))
    }
}

/// `/question decline [id]` (`/decline`): declines questions put to the user.
#[derive(Debug)]
pub(super) struct DeclineCommand(CommandDescription);

impl DeclineCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("question decline"),
            aliases: vec![CommandName::fixed("decline")],
            description:
                "Declines the questions asked of you, or those put to you whose id begins so."
                    .to_string(),
            usage: Some("[id]".to_string()),
            owners_only: false,
        })
    }
}

impl Command for DeclineCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for DeclineCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.decline_questions(args).await)
    }
}
