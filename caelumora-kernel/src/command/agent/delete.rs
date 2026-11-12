//! The built-in `/agent delete` command (issue 560).

use super::session_agents;
use crate::agent::AgentManager;
use crate::command::{command_error, named_one, CommandContext};
use crate::view::AgentView;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const DELETE_COMMAND_NAME: &str = "agent delete";

/// `/agent delete <agent>`: deletes an agent of the session.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(DELETE_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Deletes the agent of this session named; it can be restored.".to_string(),
        usage: Some("<agent>".to_string()),
        owners_only: false,
    }
}

/// Runs `/agent delete` with `args`, deleting with `agents`: the agent named - among those
/// `/agent list` lists (`SessionView::agents`), by name or handle (`named_one`) - is deleted
/// (`AgentManager::delete`, which asks `DeleteAgent`). One taking part rests in its channels
/// without an actor until it is restored, and the channels tell that it left. Only the issuer is
/// answered.
pub(super) async fn execute(
    agents: &AgentManager,
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: DELETE_COMMAND_NAME.to_string(),
        reason,
    };
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(invalid("name the agent to delete".to_string()));
    }
    let (session, found) = session_agents(DELETE_COMMAND_NAME, context).await?;
    let place = "among the agents of this session";
    let agent = named_one(DELETE_COMMAND_NAME, wanted, found, AgentView::name, place)?;
    let deleted = agents
        .delete(session.views().context(), agent.id())
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
