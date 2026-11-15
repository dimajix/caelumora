//! The kind of the commands an agent carries out itself (issues 269, 583) - `/skill`, `/compact` -,
//! which its components contribute: an agent's, beside the channel's commands
//! (`caelumora_kernel::command`), which the kernel keeps (issue 370). Who may ask an agent for one
//! is decided at its channel's door (issue 516).

use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::CommandOutput;

/// A command an agent carries out itself (issues 269, 583) - `/skill`, `/compact` -, of agents run
/// here and elsewhere alike: given the turn the agent carries it out in, as a tool is given the
/// turn it is called in. What it changes it stages on the turn, which is taken over when it
/// succeeded and dropped when it did not. Whom it was asked by is the turn's initiator. Its
/// components contribute it through `CommandProvider<dyn AgentCommand>`.
#[async_trait]
pub trait AgentCommand: Command {
    /// Carries the command out with `args`, the rest of the input line after its name and the
    /// agent's handle, which it parses itself; a command that names no parameters is never given
    /// any (`check_arguments`).
    ///
    /// # Errors
    /// As `ChannelCommand::execute`.
    async fn execute(&self, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput>;
}
