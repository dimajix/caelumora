//! The built-in `/wakeup` command (issue 28).

use crate::command::{
    command_error, named_participant, names, ChannelCommandProvider, CommandContext,
};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, WAKEUP_KIND};
use std::sync::Arc;

pub const WAKEUP_COMMAND_NAME: &str = "wakeup";

/// `/wakeup [agent]`: wakes the sleeping agents of the channel, or one of
/// them. Everybody in the channel is told, since it changes who answers.
#[derive(Debug)]
pub struct WakeupComponent;

impl ProvidesCommands for WakeupComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: WAKEUP_COMMAND_NAME.to_string(),
            description: "Wakes the sleeping agents of this channel, or the one named.".to_string(),
            usage: Some("[agent]".to_string()),
            owners_only: false,
        }]
    }
}

#[async_trait]
impl ChannelCommandProvider for WakeupComponent {
    async fn execute(
        &self,
        _name: &str,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        if wanted.is_empty() {
            let mut woken = context
                .channel
                .wakeup(&context.issuer.security_context())
                .await
                .map_err(|e| command_error(WAKEUP_COMMAND_NAME, &e))?;
            woken.sort();
            let text = if woken.is_empty() {
                "No agent was asleep.".to_string()
            } else {
                format!("Woke {}.", names(&woken))
            };
            return Ok(CommandOutput::channel(text));
        }

        // Anyone taking part, as `/nudge` takes them; only an agent sleeps.
        let participant = named_participant(WAKEUP_COMMAND_NAME, wanted, context.channel)?;
        let was_asleep = context
            .channel
            .wakeup_actor(&context.issuer.security_context(), participant.id)
            .await
            .map_err(|e| command_error(WAKEUP_COMMAND_NAME, &e))?;
        let text = if was_asleep {
            format!("Woke {}.", participant.name)
        } else {
            format!("{} was not asleep.", participant.name)
        };
        Ok(CommandOutput::channel(text))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(WAKEUP_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(WakeupComponent));
            Ok(facets)
        },
    );
}
