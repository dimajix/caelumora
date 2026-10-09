//! The built-in `/wakeup` command (issue 28).

use crate::command::{CommandContext, CommandError, CommandResult, ProvidesCommands, ChannelCommandProvider};
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
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

fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
    }
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
        let wanted = args.trim().trim_start_matches('@');
        if wanted.is_empty() {
            let mut woken = context.channel.wakeup(context.issuer).await.map_err(channel_error)?;
            woken.sort();
            let text = if woken.is_empty() {
                "No agent was asleep.".to_string()
            } else {
                format!("Woke {}.", names(&woken))
            };
            return Ok(CommandOutput::channel(text));
        }

        let agents: Vec<_> = context.channel.members().into_iter()
            .filter(|m| matches!(m.id, ActorId::Agent(_)))
            .collect();
        let Some(agent) = agents.iter().find(|m| m.handle.as_deref() == Some(wanted)) else {
            let mut handles: Vec<String> = agents.iter().filter_map(|m| m.handle.clone()).map(|h| format!("@{h}")).collect();
            handles.sort();
            let reason = if handles.is_empty() {
                "there are no agents in this channel".to_string()
            } else {
                format!("no agent '{}' in this channel; the agents are {}", wanted, names(&handles))
            };
            return Err(CommandError::InvalidArguments { command: WAKEUP_COMMAND_NAME.to_string(), reason });
        };

        let was_asleep = context.channel.wakeup_actor(context.issuer, agent.id).await.map_err(channel_error)?;
        let text = if was_asleep {
            format!("Woke {}.", agent.name)
        } else {
            format!("{} was not asleep.", agent.name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_joined_as_a_sentence() {
        assert_eq!(names(&[]), "");
        assert_eq!(names(&["Sassy".to_string()]), "Sassy");
        assert_eq!(names(&["A".to_string(), "B".to_string(), "C".to_string()]), "A, B and C");
    }
}
