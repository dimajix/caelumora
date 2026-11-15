//! The terminal's commands on the calls waiting for approval (issues 570, 571): `/approval approve`
//! and `deny`.

use super::TerminalCommand;
use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::Command;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;

/// `/approval approve [id]` (`/approve`) and `/approval deny [id]` (`/deny`): approve or deny a
/// call waiting for approval, the one asked or one named - one type, as they differ in nothing
/// but the answer.
#[derive(Debug)]
pub(super) struct ApprovalCommand {
    description: CommandDescription,
    /// Whether it approves the call, rather than denies it.
    approved: bool,
}

impl ApprovalCommand {
    /// `/approval approve`, as `/help` tells it.
    pub(super) fn approve() -> Self {
        Self {
            description: CommandDescription {
                name: CommandName::fixed("approval approve"),
                aliases: vec![CommandName::fixed("approve")],
                description: "Approves the call asked of you, or the one waiting for approval whose id begins so.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
            approved: true,
        }
    }

    /// `/approval deny`, as `/help` tells it.
    pub(super) fn deny() -> Self {
        Self {
            description: CommandDescription {
                name: CommandName::fixed("approval deny"),
                aliases: vec![CommandName::fixed("deny")],
                description: "Denies the call asked of you, or the one waiting for approval whose id begins so.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
            approved: false,
        }
    }
}

impl Command for ApprovalCommand {
    fn description(&self) -> &CommandDescription {
        &self.description
    }
}

#[async_trait]
impl TerminalCommand for ApprovalCommand {
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled {
        Handled::Printed(talk.answer_approval(args, self.approved).await)
    }
}
