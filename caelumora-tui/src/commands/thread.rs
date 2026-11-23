//! The terminal's commands on the threads of the channel talked in (issue 595): `/thread list`,
//! `/thread follow` and `/thread line`.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::{CommandDescription, CommandParameter};
use caelumora_domain::value::command_name::CommandName;

/// `/thread list`: lists the threads open in the channel talked in, numbered.
#[derive(Debug)]
pub(super) struct ListCommand(CommandDescription);

impl ListCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("thread list"),
            aliases: Vec::new(),
            description: "Lists the threads open in this channel.".to_string(),
            parameters: Vec::new(),
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
impl TerminalCommand for ListCommand {
    async fn execute(&self, _args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.list_threads().await)
    }
}

/// `/thread follow <thread>`: follows a thread of the channel talked in - by its number in
/// `/thread list`, or the beginning of its id -: what it shows is printed, and what is typed goes
/// there.
#[derive(Debug)]
pub(super) struct FollowCommand(CommandDescription);

impl FollowCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("thread follow"),
            aliases: Vec::new(),
            description: "Follows a thread of this channel: by its number in /thread list, or the \
                          beginning of its id."
                .to_string(),
            parameters: vec![CommandParameter::required("thread")],
            owners_only: false,
        })
    }
}

impl Command for FollowCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for FollowCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.follow_thread(args).await)
    }
}

/// `/thread line`: follows the line of the channel talked in again.
#[derive(Debug)]
pub(super) struct LineCommand(CommandDescription);

impl LineCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("thread line"),
            aliases: Vec::new(),
            description: "Follows the line of this channel again.".to_string(),
            parameters: Vec::new(),
            owners_only: false,
        })
    }
}

impl Command for LineCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for LineCommand {
    async fn execute(&self, _args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.follow_line().await)
    }
}
