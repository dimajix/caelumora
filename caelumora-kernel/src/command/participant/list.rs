//! The built-in `/participant list` command (issue 560).

use crate::command::{command_error, taking_part, ChannelCommand, CommandContext};
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

const LIST_COMMAND_NAME: &str = "participant list";

/// `/participant list`: who takes part in the channel.
#[derive(Debug)]
pub(super) struct ListCommand(CommandDescription);

impl ListCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(LIST_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Lists who takes part in this channel.".to_string(),
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
    /// Runs `/participant list`: who takes part in the channel now, as the issuer reads it
    /// (`ChannelView::participants`) - a line each, with the handle, whether a user or an agent,
    /// and an agent's personality. Only the issuer is answered.
    async fn execute(
        &self,
        _args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let failed = |e: Error| command_error(LIST_COMMAND_NAME, &e);
        let mut text = String::from("Taking part in this channel:\n\n");
        for participant in taking_part(LIST_COMMAND_NAME, context).await? {
            let name = participant.name().map_or("someone who no longer exists", Name::as_str);
            let handle = participant.handle().map(|h| format!(" (@{h})")).unwrap_or_default();
            let kind = match participant.id() {
                ActorId::User(_) => "user".to_string(),
                ActorId::Agent(_) => {
                    match participant.personality().await.map_err(|e| failed(e.into()))? {
                        Some((personality, _)) => format!("agent of {personality}"),
                        None => "agent".to_string(),
                    }
                }
                // A channel's participants are users and agents alone (`Participant`).
                ActorId::Approver(_) | ActorId::System | ActorId::None => "participant".to_string(),
            };
            text.push_str(&format!("- {name}{handle}, {kind}\n"));
        }
        Ok(CommandOutput::private(text))
    }
}
