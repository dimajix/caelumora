//! The built-in `/nudge` command (issue 278).

use crate::channel::ChannelError;
use crate::command::{command_error, named_participant, ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, NUDGE_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

pub const NUDGE_COMMAND_NAME: &str = "nudge";

/// `/nudge <participant>`: asks someone in the channel - an agent, another
/// user - to respond to the conversation as it stands, without a message -
/// as an `@`-mention would, with nothing said (`UserActor::request_response`).
/// Only the issuer is told, and the one asked: the history keeps nothing of
/// it, and the others see the answer.
#[derive(Debug)]
pub struct NudgeComponent;

impl ProvidesCommands for NudgeComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: CommandName::fixed(NUDGE_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Asks the participant named to respond to the conversation as it stands, without a message.".to_string(),
            usage: Some("<participant>".to_string()),
            owners_only: false,
        }]
    }
}

#[async_trait]
impl ChannelCommandProvider for NudgeComponent {
    async fn execute(
        &self,
        _name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        if wanted.is_empty() {
            return Err(CommandError::InvalidArguments {
                command: NUDGE_COMMAND_NAME.to_string(),
                reason: "name who is to respond".to_string(),
            });
        }
        let participant = named_participant(NUDGE_COMMAND_NAME, wanted, context.channel)?;
        match context.issuer.request_response(participant.id).await {
            Ok(_) => Ok(CommandOutput::private(format!("Asked {} to respond.", participant.name))),
            Err(e) => match e.downcast_ref::<ChannelError>() {
                Some(ChannelError::AlreadyResponding { .. }) => Ok(CommandOutput::private(
                    format!("{} is responding already.", participant.name),
                )),
                Some(ChannelError::NudgesItself { .. }) => Err(CommandError::InvalidArguments {
                    command: NUDGE_COMMAND_NAME.to_string(),
                    reason: "you cannot nudge yourself".to_string(),
                }),
                _ => Err(command_error(NUDGE_COMMAND_NAME, &e)),
            },
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(NUDGE_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(NudgeComponent));
            Ok(facets)
        },
    );
}
