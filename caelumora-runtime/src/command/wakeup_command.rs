//! The built-in `/wakeup` command (issue 28).

use crate::command::{named_participant, names, CommandContext, CommandError, CommandResult, ProvidesCommands, ChannelCommandProvider};
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, WAKEUP_COMMAND_KIND};
use std::sync::Arc;

pub const WAKEUP_COMMAND_NAME: &str = "wakeup";

/// `/wakeup [agent]`: wakes the sleeping agents of the channel, or one of
/// them. Everybody in the channel is told, since it changes who answers.
#[derive(Debug)]
pub struct WakeupCommandProvider;

/// A refusal from the channel is the command being refused; anything else is
/// a failure, described only as far as `user_message` allows.
fn channel_error(error: anyhow::Error) -> CommandError {
    if crate::error::is_forbidden(&error) {
        return CommandError::Forbidden(WAKEUP_COMMAND_NAME.to_string());
    }
    tracing::warn!("/wakeup failed: {:#}", error);
    let reason = crate::error::user_message(&error).unwrap_or_else(|| "see the server log".to_string());
    CommandError::Failed { command: WAKEUP_COMMAND_NAME.to_string(), reason }
}

impl ProvidesCommands for WakeupCommandProvider {
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
impl ChannelCommandProvider for WakeupCommandProvider {
    async fn execute(&self, _name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        if wanted.is_empty() {
            let mut woken = context.channel.wakeup(&context.issuer.security_context()).await.map_err(channel_error)?;
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
        let was_asleep = context.channel.wakeup_actor(&context.issuer.security_context(), participant.id).await.map_err(channel_error)?;
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
        ComponentKind::new(WAKEUP_COMMAND_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(WakeupCommandProvider));
            Ok(facets)
        },
    );
}

