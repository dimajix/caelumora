//! The commands an agent carries out itself (issue 269), as a facet of its components - `/skill`,
//! `/compact` -: an agent's, beside the channel's commands (`caelumora_kernel::command`), which the
//! kernel keeps (issue 370). Who may ask an agent for one is decided at its channel's door (issue
//! 516).

use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::command::{CommandResult, ProvidesCommands};
use caelumora_domain::command::CommandOutput;

/// A component facet contributing commands an agent carries out itself
/// (issue 269) - `/skill`, `/compact` -, of agents run here and elsewhere
/// alike: given the turn the agent carries the command out in, as a tool is
/// given the turn it is called in. What the command changes it stages on
/// the turn, which is taken over when the command succeeded and dropped
/// when it did not. Whom it was asked by is the turn's initiator.
#[async_trait]
pub trait AgentCommandProvider: ProvidesCommands {
    /// Carries out the command `name` with `args`, the rest of the input
    /// line after the name and the agent's handle, which the command parses
    /// itself.
    ///
    /// # Errors
    /// As `ChannelCommandProvider::execute`.
    async fn execute(
        &self,
        name: &str,
        args: &str,
        turn: &mut Turn<'_>,
    ) -> CommandResult<CommandOutput>;
}
