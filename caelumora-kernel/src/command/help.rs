//! The built-in `/help` command (issues 28, 560).

use crate::command::{ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, HELP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const HELP_COMMAND_NAME: &str = "help";

/// `/help [group or command]`: lists the commands the user may see - all, or those of a group
/// (issue 560) -, or describes one, found by its name or a short form. The answer is for the user
/// alone.
#[derive(Debug)]
pub struct HelpComponent;

fn signature(command: &Command) -> String {
    match &command.usage {
        Some(usage) => format!("/{} {}", command.name, usage),
        None => format!("/{}", command.name),
    }
}

/// The short forms of `command`, as told after its signature: " (also `/add`)", or nothing.
fn also(command: &Command) -> String {
    if command.aliases.is_empty() {
        return String::new();
    }
    let aliases: Vec<String> = command.aliases.iter().map(|alias| format!("`/{alias}`")).collect();
    format!(" (also {})", aliases.join(", "))
}

impl ProvidesCommands for HelpComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: CommandName::fixed(HELP_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Lists the commands you can use, those of a group, or describes one."
                .to_string(),
            usage: Some("[group or command]".to_string()),
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
async fn agents_commands(
    context: &CommandContext<'_>,
) -> BTreeMap<CommandName, (Command, Vec<String>)> {
    let mut by_name: BTreeMap<CommandName, (Command, Vec<String>)> = BTreeMap::new();
    for (agent, commands) in
        context.channel.agent_commands(&context.issuer.security_context()).await
    {
        let Some(handle) = agent.handle else { continue };
        for command in commands {
            by_name
                .entry(command.name.clone())
                .or_insert_with(|| (command, Vec::new()))
                .1
                .push(format!("@{handle}"));
        }
    }
    by_name
}

#[async_trait]
impl ChannelCommandProvider for HelpComponent {
    async fn execute(
        &self,
        _name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let visible = context
            .executor
            .visible(&context.issuer.security_context(), &context.channel.security_attributes())
            .await;
        let of_agents = agents_commands(context).await;
        let wanted = args.trim().trim_start_matches('/');
        // Unknown and invisible alike, as for running a command.
        let unknown = || CommandError::Unknown(wanted.to_string());
        if !wanted.is_empty() {
            // By its name or a short form.
            let named = |command: &Command| command.strip_from(wanted) == Some("");
            if let Some(command) = visible.iter().find(|c| named(c)) {
                return Ok(CommandOutput::private(format!(
                    "`{}`{} — {}",
                    signature(command),
                    also(command),
                    command.description
                )));
            }
            if let Some((command, handles)) = of_agents.values().find(|(c, _)| named(c)) {
                return Ok(CommandOutput::private(format!(
                    "`{}`{} — {} Carried out by {}.",
                    agent_signature(command),
                    also(command),
                    command.description,
                    handles.join(", ")
                )));
            }
        }

        // All of them, or those of the group asked for: whose name begins with it.
        let group = match wanted {
            "" => None,
            _ => {
                let words: Vec<&str> = wanted.split_whitespace().collect();
                Some(CommandName::new(words.join(" ")).map_err(|_| unknown())?)
            }
        };
        let in_group = |command: &Command| group.as_ref().is_none_or(|g| g.begins(&command.name));
        let listed: Vec<&Command> = visible.iter().filter(|c| in_group(c)).collect();
        let of_agents: Vec<_> = of_agents.values().filter(|(c, _)| in_group(c)).collect();
        if listed.is_empty() && of_agents.is_empty() {
            return Err(unknown());
        }
        let mut text = match &group {
            Some(group) => format!("Commands of `/{group}`:\n\n"),
            None => String::from("Commands:\n\n"),
        };
        for command in listed {
            text.push_str(&format!(
                "- `{}`{} — {}\n",
                signature(command),
                also(command),
                command.description
            ));
        }
        if !of_agents.is_empty() {
            text.push_str("\nCommands the agents here carry out - name the agent, or `@all`:\n\n");
            for (command, handles) in of_agents {
                text.push_str(&format!(
                    "- `{}`{} — {} ({})\n",
                    agent_signature(command),
                    also(command),
                    command.description,
                    handles.join(", ")
                ));
            }
        }
        text.push_str("\nStart a message with `//` to send it with a leading slash.");
        Ok(CommandOutput::private(text))
    }
}

/// Registers the kind `help` with `registry`.
pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(HELP_KIND).provides::<dyn ChannelCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(HelpComponent));
            Ok(facets)
        },
    );
}
