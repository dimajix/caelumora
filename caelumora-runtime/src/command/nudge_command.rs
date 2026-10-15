//! The built-in `/nudge` command (issue 278).

use crate::channel::ChannelError;
use crate::command::{named_participant, CommandContext, CommandError, CommandResult, ProvidesCommands, ChannelCommandProvider};
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, NUDGE_COMMAND_KIND};
use std::sync::Arc;
use crate::error::user_message;

pub const NUDGE_COMMAND_NAME: &str = "nudge";

/// `/nudge <participant>`: asks someone in the channel - an agent, another
/// user - to respond to the conversation as it stands, without a message -
/// as an `@`-mention would, with nothing said (`UserActor::request_response`).
/// Only the issuer is told, and the one asked: the history keeps nothing of
/// it, and the others see the answer.
#[derive(Debug)]
pub struct NudgeCommandProvider;

impl ProvidesCommands for NudgeCommandProvider {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: NUDGE_COMMAND_NAME.to_string(),
            description: "Asks the participant named to respond to the conversation as it stands, without a message.".to_string(),
            usage: Some("<participant>".to_string()),
            owners_only: false,
        }]
    }
}

#[async_trait]
impl ChannelCommandProvider for NudgeCommandProvider {
    async fn execute(&self, _name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        if wanted.is_empty() {
            return Err(CommandError::InvalidArguments { command: NUDGE_COMMAND_NAME.to_string(), reason: "name who is to respond".to_string() });
        }
        let participant = named_participant(NUDGE_COMMAND_NAME, wanted, context.channel)?;
        match context.issuer.request_response(participant.id).await {
            Ok(_) => Ok(CommandOutput::private(format!("Asked {} to respond.", participant.name))),
            Err(e) => match e.downcast_ref::<ChannelError>() {
                Some(ChannelError::AlreadyResponding { .. }) => Ok(CommandOutput::private(format!("{} is responding already.", participant.name))),
                Some(ChannelError::NudgesItself { .. }) => Err(CommandError::InvalidArguments { command: NUDGE_COMMAND_NAME.to_string(), reason: "you cannot nudge yourself".to_string() }),
                _ => {
                    tracing::warn!("/nudge failed: {:#}", e);
                    let reason = user_message(&e).unwrap_or_else(|| "see the server log".to_string());
                    Err(CommandError::Failed { command: NUDGE_COMMAND_NAME.to_string(), reason })
                }
            },
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(NUDGE_COMMAND_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(NudgeCommandProvider));
            Ok(facets)
        },
    );
}
