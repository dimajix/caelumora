//! The built-in `/agent delete` command (issue 560).

use super::session_agents;
use crate::agent::AgentManager;
use crate::command::{command_error, named_one, ChannelCommand, CommandContext};
use crate::view::AgentView;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

const DELETE_COMMAND_NAME: &str = "agent delete";

/// `/agent delete <agent>`: deletes an agent of the session.
#[derive(Debug)]
pub(super) struct DeleteCommand {
    description: CommandDescription,
    agents: Arc<AgentManager>,
}

impl DeleteCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new(agents: Arc<AgentManager>) -> Self {
        let description = CommandDescription {
            name: CommandName::fixed(DELETE_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Deletes the agent of this session named; it can be restored.".to_string(),
            parameters: vec![CommandParameter::required("agent")],
            owners_only: false,
        };
        Self { description, agents }
    }
}

impl Command for DeleteCommand {
    fn description(&self) -> &CommandDescription {
        &self.description
    }
}

#[async_trait]
impl ChannelCommand for DeleteCommand {
    /// Runs `/agent delete` with `args`, deleting with `agents`: the agent named - among those
    /// `/agent list` lists (`SessionView::agents`), by name or handle (`named_one`) - is deleted
    /// (`AgentManager::delete`, which asks `DeleteAgent`). One taking part rests in its channels
    /// without an actor until it is restored, and the channels tell that it left. Only the issuer
    /// is answered.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let agents = self.agents.as_ref();
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: DELETE_COMMAND_NAME.to_string(),
            reason,
        };
        let wanted = args.trim();
        let (session, found) = session_agents(DELETE_COMMAND_NAME, context).await?;
        let place = "among the agents of this session";
        let agent = named_one(DELETE_COMMAND_NAME, wanted, found, AgentView::name, place)?;
        let deleted = agents
            .delete(&context.issuer.security_context(), agent.id())
            .await
            .map_err(|e| command_error(DELETE_COMMAND_NAME, &e.into()))?;
        if !deleted {
            // Gone since it was found.
            return Err(invalid(format!("no agent '{wanted}' in this session")));
        }
        let took_part = session.as_session().takes_part(ActorId::Agent(agent.id()));
        Ok(CommandOutput::private(match took_part {
            true => format!(
                "Deleted {}; it rests in the channels it took part in until it is restored.",
                agent.name()
            ),
            false => format!("Deleted {}; it can be restored.", agent.name()),
        }))
    }
}
