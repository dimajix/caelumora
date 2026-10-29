//! The commands an agent carries out itself (issue 269), as a facet of its
//! components - `/skill`, `/compact` -, and what the command executor asks
//! of them: an agent's, beside the channel's commands
//! (`caelumora_runtime::command`), which the runtime keeps (issue 370).

use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext};
use caelumora_runtime::command::{CommandEntry, CommandExecutor, CommandResult, ProvidesCommands};

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

/// The commands of an agent (issue 269): seen and asked for by
/// `ShowAgentCommand` and `ExecuteAgentCommand`, with the agent's
/// attributes and those of the channel it is asked in - what the executor
/// asks of an agent's commands, said by their owner (issue 370).
#[async_trait]
pub trait AgentCommandAccess {
    /// The commands `issuer` may see of the agent with `agent` attributes in
    /// the channel with `channel` attributes, sorted by name.
    async fn visible(
        &self,
        issuer: &SecurityContext,
        agent: &SecurityAttributes,
        channel: &SecurityAttributes,
    ) -> Vec<Command>;

    /// The command `name`, if `issuer` - who asked, in the context the
    /// request came with - may ask the agent with `agent` attributes for it
    /// in the channel with `channel` attributes: ready to be carried out,
    /// and only so.
    ///
    /// # Errors
    /// `CommandError::Empty` for an empty name; `Unknown` if the agent has
    /// no such command or the issuer may not see it; `Forbidden` if they
    /// may see but not ask for it.
    async fn admitted<'c>(
        &'c self,
        name: &str,
        issuer: &SecurityContext,
        agent: &'c SecurityAttributes,
        channel: &'c SecurityAttributes,
    ) -> CommandResult<AdmittedCommand<'c>>;
}

#[async_trait]
impl AgentCommandAccess for CommandExecutor<dyn AgentCommandProvider> {
    async fn visible(
        &self,
        issuer: &SecurityContext,
        agent: &SecurityAttributes,
        channel: &SecurityAttributes,
    ) -> Vec<Command> {
        self.visible_by(issuer, |command| ActionType::ShowAgentCommand { command, agent, channel })
            .await
    }

    async fn admitted<'c>(
        &'c self,
        name: &str,
        issuer: &SecurityContext,
        agent: &'c SecurityAttributes,
        channel: &'c SecurityAttributes,
    ) -> CommandResult<AdmittedCommand<'c>> {
        let entry = self
            .admit(
                name,
                issuer,
                |command| ActionType::ShowAgentCommand { command, agent, channel },
                |command| ActionType::ExecuteAgentCommand { command, agent, channel },
            )
            .await?;
        Ok(AdmittedCommand(entry))
    }
}

/// A command of an agent its issuer may ask for (`AgentCommandAccess::admitted`):
/// what it is, and carrying it out - which can only be done once it was
/// admitted, so that the agent can tell the issuer it began in between
/// (issue 299).
#[derive(Debug)]
pub struct AdmittedCommand<'c>(&'c CommandEntry<dyn AgentCommandProvider>);

impl AdmittedCommand<'_> {
    pub fn command(&self) -> &Command {
        self.0.command()
    }

    /// Carries the command out with `args` in `turn`.
    ///
    /// # Errors
    /// Whatever the command reports.
    pub async fn execute(self, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        self.0.provider().execute(&self.0.command().name, args, turn).await
    }
}
