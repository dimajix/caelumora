//! The built-in `/help` command (issues 28, 560).

use crate::command::{ChannelCommand, CommandContext};
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandProvider, CommandResult};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{CommandDescription, CommandOutput, HELP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const HELP_COMMAND_NAME: &str = "help";

/// `/help [group or command]`: lists the commands the user may see - all, or those of a group
/// (issue 560) -, or describes one, found by its name or a short form. The answer is for the user
/// alone.
#[derive(Debug)]
pub struct HelpCommand(CommandDescription);

impl HelpCommand {
    /// The command, as `/help` tells it.
    pub fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(HELP_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Lists the commands you can use, those of a group, or describes one."
                .to_string(),
            usage: Some("[group or command]".to_string()),
            owners_only: false,
        })
    }
}

impl Default for HelpCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl Command for HelpCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

/// The commands the agents of the channel carry out that the issuer may
/// see (issue 269), by name, each with the handles of the agents that do.
async fn agents_commands(
    context: &CommandContext<'_>,
) -> BTreeMap<CommandName, (CommandDescription, Vec<String>)> {
    let mut by_name: BTreeMap<CommandName, (CommandDescription, Vec<String>)> = BTreeMap::new();
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
impl ChannelCommand for HelpCommand {
    async fn execute(
        &self,
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
            let named = |command: &CommandDescription| command.strip_from(wanted) == Some("");
            if let Some(command) = visible.iter().find(|c| named(c)) {
                return Ok(CommandOutput::private(command.help_line()));
            }
            if let Some((command, handles)) = of_agents.values().find(|(c, _)| named(c)) {
                return Ok(CommandOutput::private(format!(
                    "{} Carried out by {}.",
                    command.agent_help_line(),
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
        let in_group =
            |command: &CommandDescription| group.as_ref().is_none_or(|g| g.begins(&command.name));
        let listed: Vec<&CommandDescription> = visible.iter().filter(|c| in_group(c)).collect();
        let of_agents: Vec<_> = of_agents.values().filter(|(c, _)| in_group(c)).collect();
        if listed.is_empty() && of_agents.is_empty() {
            return Err(unknown());
        }
        let mut text = match &group {
            Some(group) => format!("Commands of `/{group}`:\n\n"),
            None => String::from("Commands:\n\n"),
        };
        for command in listed {
            text.push_str(&format!("- {}\n", command.help_line()));
        }
        if !of_agents.is_empty() {
            text.push_str("\nCommands the agents here carry out - name the agent, or `@all`:\n\n");
            for (command, handles) in of_agents {
                text.push_str(&format!(
                    "- {} ({})\n",
                    command.agent_help_line(),
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
        ComponentKind::new(HELP_KIND).provides::<dyn CommandProvider<dyn ChannelCommand>>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            let help: Vec<Arc<dyn ChannelCommand>> = vec![Arc::new(HelpCommand::new())];
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(help));
            Ok(facets)
        },
    );
}
