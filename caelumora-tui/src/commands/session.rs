//! The terminal's commands on the sessions of the directory (issues 570, 571): `/session list`,
//! `new`, `resume` and `delete`.

use super::TerminalCommand;
use crate::sessions::Opening;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;

/// `/session list`: lists the sessions of the directory.
#[derive(Debug)]
pub(super) struct ListCommand(CommandDescription);

impl ListCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("session list"),
            aliases: Vec::new(),
            description: "Lists the sessions of this directory, the one spoken in last first."
                .to_string(),
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
impl TerminalCommand for ListCommand {
    async fn execute(&self, _args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.listed().await)
    }
}

/// `/session new [template]` (`/new`): starts a session and talks in it.
#[derive(Debug)]
pub(super) struct NewCommand(CommandDescription);

impl NewCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("session new"),
            aliases: vec![CommandName::fixed("new")],
            description:
                "Starts a session from the template named, or the program's, and talks in it."
                    .to_string(),
            usage: Some("[template]".to_string()),
            owners_only: false,
        })
    }
}

impl Command for NewCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for NewCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        let template = (!args.is_empty()).then(|| args.to_string());
        Handled::Printed(talk.switch_session(&Opening::Start { template }).await)
    }
}

/// `/session resume <id>` (`/resume`): continues a session of the directory and talks in it.
#[derive(Debug)]
pub(super) struct ResumeCommand(CommandDescription);

impl ResumeCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("session resume"),
            aliases: vec![CommandName::fixed("resume")],
            description:
                "Continues the session of this directory whose id begins so, and talks in it."
                    .to_string(),
            usage: Some("<id>".to_string()),
            owners_only: false,
        })
    }
}

impl Command for ResumeCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl TerminalCommand for ResumeCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.switch_session(&Opening::Resume(args.to_string())).await)
    }
}

/// `/session delete <id>`: deletes a session of the directory.
#[derive(Debug)]
pub(super) struct DeleteCommand(CommandDescription);

impl DeleteCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed("session delete"),
            aliases: Vec::new(),
            description: "Deletes the session of this directory whose id begins so, not the one talked in; it can be restored.".to_string(),
            usage: Some("<id>".to_string()),
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
impl TerminalCommand for DeleteCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.delete_session(args).await)
    }
}
