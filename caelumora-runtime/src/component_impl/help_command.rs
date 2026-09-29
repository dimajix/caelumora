//! The built-in `/help` command (issue 28).

use crate::command::{CommandContext, CommandError, CommandProvider, CommandResult};
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, HELP_COMMAND_KIND};
use std::sync::Arc;

const HELP: &str = "help";

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

#[async_trait]
impl CommandProvider for HelpCommandProvider {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: HELP.to_string(),
            description: "Lists the commands you can use, or describes one.".to_string(),
            usage: Some("[command]".to_string()),
        }]
    }

    async fn execute(&self, _name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        let visible = context.executor.visible(context.issuer).await;
        let wanted = args.trim().trim_start_matches('/');
        if !wanted.is_empty() {
            // Unknown and invisible alike, as for running a command.
            let command = visible.iter().find(|c| c.name == wanted)
                .ok_or_else(|| CommandError::Unknown(wanted.to_string()))?;
            return Ok(CommandOutput::private(format!("`{}` — {}", signature(command), command.description)));
        }

        let mut text = String::from("Commands:\n\n");
        for command in &visible {
            text.push_str(&format!("- `{}` — {}\n", signature(command), command.description));
        }
        text.push_str("\nStart a message with `//` to send it with a leading slash.");
        Ok(CommandOutput::private(text))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(HELP_COMMAND_KIND).provides::<dyn CommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider>(Arc::new(HelpCommandProvider));
            Ok(facets)
        },
    );
}
