//! The built-in `/help` command (issue 28).

use crate::command::{CommandContext, CommandError, CommandResult, ProvidesCommands, ChannelCommandProvider};
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, HELP_COMMAND_KIND};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const HELP_COMMAND_NAME: &str = "help";

/// `/help [command]`: lists the commands the user may see, or describes one.
/// The answer is for the user alone.
#[derive(Debug)]
pub struct HelpCommandProvider;

fn signature(command: &Command) -> String {
    match &command.usage {
        Some(usage) => format!("/{} {}", command.name, usage),
        None => format!("/{}", command.name),
    }
}

impl ProvidesCommands for HelpCommandProvider {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: HELP_COMMAND_NAME.to_string(),
            description: "Lists the commands you can use, or describes one.".to_string(),
            usage: Some("[command]".to_string()),
            owners_only: false,
        }]
    }
}

/// How a command of an agent is written (issue 269): with the agent named.
fn agent_signature(command: &Command) -> String {
    match &command.usage {
        Some(usage) => format!("/{} @agent {}", command.name, usage),
        None => format!("/{} @agent", command.name),
    }
}

/// The commands the agents of the channel carry out that the issuer may
/// see (issue 269), by name, each with the handles of the agents that do.
async fn agents_commands(context: &CommandContext<'_>) -> BTreeMap<String, (Command, Vec<String>)> {
    let mut by_name: BTreeMap<String, (Command, Vec<String>)> = BTreeMap::new();
    for (member, commands) in context.channel.agent_commands(&context.issuer.security_context()).await {
        let Some(handle) = member.handle else { continue };
        for command in commands {
            by_name.entry(command.name.clone()).or_insert_with(|| (command, Vec::new())).1.push(format!("@{handle}"));
        }
    }
    by_name
}

#[async_trait]
impl ChannelCommandProvider for HelpCommandProvider {
    async fn execute(&self, _name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        let visible = context.executor.visible(&context.issuer.security_context(), &context.channel.security_attributes()).await;
        let of_agents = agents_commands(context).await;
        let wanted = args.trim().trim_start_matches('/');
        if !wanted.is_empty() {
            // Unknown and invisible alike, as for running a command.
            if let Some(command) = visible.iter().find(|c| c.name == wanted) {
                return Ok(CommandOutput::private(format!("`{}` — {}", signature(command), command.description)));
            }
            let (command, handles) = of_agents.get(wanted).ok_or_else(|| CommandError::Unknown(wanted.to_string()))?;
            return Ok(CommandOutput::private(format!("`{}` — {} Carried out by {}.", agent_signature(command), command.description, handles.join(", "))));
        }

        let mut text = String::from("Commands:\n\n");
        for command in &visible {
            text.push_str(&format!("- `{}` — {}\n", signature(command), command.description));
        }
        if !of_agents.is_empty() {
            text.push_str("\nCommands the agents here carry out - name the agent, or `@all`:\n\n");
            for (command, handles) in of_agents.values() {
                text.push_str(&format!("- `{}` — {} ({})\n", agent_signature(command), command.description, handles.join(", ")));
            }
        }
        text.push_str("\nStart a message with `//` to send it with a leading slash.");
        Ok(CommandOutput::private(text))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(HELP_COMMAND_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(HelpCommandProvider));
            Ok(facets)
        },
    );
}
