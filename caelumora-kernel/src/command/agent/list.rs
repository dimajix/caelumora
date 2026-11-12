//! The built-in `/agent list` command (issue 560).

use super::session_agents;
use crate::command::{command_error, CommandContext};
use anyhow::Error;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const LIST_COMMAND_NAME: &str = "agent list";

/// `/agent list`: the agents of the session.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(LIST_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Lists the agents of this session: those taking part, and those made here."
            .to_string(),
        usage: None,
        owners_only: false,
    }
}

/// Runs `/agent list`: the agents of the session the issuer may read (`SessionView::agents`), a
/// line each, with the handle, the personality, and whether it takes part in this channel, in
/// another of the session or in none. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    if !args.trim().is_empty() {
        return Err(CommandError::InvalidArguments {
            command: LIST_COMMAND_NAME.to_string(),
            reason: "it takes no arguments".to_string(),
        });
    }
    let failed = |e: Error| command_error(LIST_COMMAND_NAME, &e);
    let (session, agents) = session_agents(LIST_COMMAND_NAME, context).await?;
    if agents.is_empty() {
        return Ok(CommandOutput::private("This session has no agents."));
    }
    let channel = context.channel.channel();
    let mut text = String::from("The agents of this session:\n\n");
    for agent in agents {
        let actor = ActorId::Agent(agent.id());
        let name = agent.name();
        let handle = agent.handle().map(|h| format!(" (@{h})")).unwrap_or_default();
        let personality = match agent.personality().await.map_err(|e| failed(e.into()))? {
            Some((personality, _)) => format!(", of {personality}"),
            None => String::new(),
        };
        let place = if channel.lists(actor) {
            "in this channel"
        } else if session.as_session().takes_part(actor) {
            "in the session"
        } else {
            "not taking part"
        };
        text.push_str(&format!("- {name}{handle}{personality}, {place}\n"));
    }
    Ok(CommandOutput::private(text))
}
