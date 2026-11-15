//! The command of `agent_memory` (issue 574): `/memory @agent`, which shows
//! the agent's owners what it recalls of its memory where the command is carried out.

use crate::extension::MemoryReader;
use crate::memory::Memory;
use async_trait::async_trait;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::turn::Turn;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

/// The name of its command, `/memory`.
const MEMORY_COMMAND_NAME: &str = "memory";

/// `/memory @agent`: shows the agent's owners what it recalls of its memory where it is carried
/// out.
#[derive(Debug)]
pub(super) struct MemoryCommand(CommandDescription);

impl MemoryCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(MEMORY_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Shows what the agent recalls of its memory here: what it remembered in this and other channels and sessions, as far as it may be told here.".to_string(),
            usage: None,
            // Its memory is its owner's (issue 518), as what it saw.
            owners_only: true,
        })
    }
}

impl Command for MemoryCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl AgentCommand for MemoryCommand {
    /// Carries out `/memory`: what the agent recalls of its memory in the channel, as its prompt
    /// part gives it - the entries the plugin hands the work through its `MemoryReader` -,
    /// answered to the issuer alone.
    async fn execute(&self, _args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        // None where the work reaches no session or its memory cannot be read.
        let memory = turn.situation().reader::<dyn MemoryReader>().await.ok_or_else(|| {
            CommandError::Failed {
                command: MEMORY_COMMAND_NAME.to_string(),
                reason: "the memory cannot be read here".to_string(),
            }
        })?;
        Ok(CommandOutput::private(listed(turn.agent_name(), memory.recalled())))
    }
}

/// What `/memory` answers of the agent `agent`, which recalls `recalled` here: each entry with its
/// number, as it changes them here.
fn listed(agent: &str, recalled: &[Memory]) -> String {
    if recalled.is_empty() {
        return format!("{agent} recalls nothing of its memory here.");
    }
    let entries: Vec<String> =
        recalled.iter().map(|entry| format!("- #{} {}", entry.id, entry.text)).collect();
    format!("What {agent} recalls of its memory here:\n{}", entries.join("\n"))
}
