//! The built-in `/participant list` command (issue 560).

use crate::command::{command_error, CommandContext};
use crate::view::Viewer;
use anyhow::Error;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const LIST_COMMAND_NAME: &str = "participant list";

/// `/participant list`: who takes part in the channel.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(LIST_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Lists who takes part in this channel.".to_string(),
        usage: None,
        owners_only: false,
    }
}

/// Runs `/participant list`: who takes part in the channel now, as the issuer reads it
/// (`ChannelView::participants`) - a line each, with the handle, whether a user or an agent, and an
/// agent's personality. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let failed = |e: Error| command_error(LIST_COMMAND_NAME, &e);
    if !args.trim().is_empty() {
        return Err(CommandError::InvalidArguments {
            command: LIST_COMMAND_NAME.to_string(),
            reason: "it takes no arguments".to_string(),
        });
    }
    let session = context
        .channel
        .session_loop()
        .ok_or_else(|| failed(Error::msg("the channel belongs to no session")))?;
    let channel = session
        .views(Viewer::new(context.issuer.security_context()))
        .session(session.id())
        .await
        .map_err(|e| failed(e.into()))?
        .channel(context.channel.id())
        .await
        .map_err(|e| failed(e.into()))?;
    let mut text = String::from("Taking part in this channel:\n\n");
    for participant in channel.participants().await {
        let name = participant.name().unwrap_or("someone who no longer exists");
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
