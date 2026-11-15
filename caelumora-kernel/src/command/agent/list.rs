//! The built-in `/agent list` command (issue 560).

use super::session_agents;
use crate::command::{command_error, ChannelCommand, CommandContext};
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

const LIST_COMMAND_NAME: &str = "agent list";

/// `/agent list`: the agents of the session.
#[derive(Debug)]
pub(super) struct ListCommand(CommandDescription);

impl ListCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(LIST_COMMAND_NAME),
            aliases: Vec::new(),
            description:
                "Lists the agents of this session: those taking part, and those made here."
                    .to_string(),
            parameters: Vec::new(),
            owners_only: false,
        })
    }
}

impl Command for ListCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for ListCommand {
    /// Runs `/agent list`: the agents of the session the issuer may read (`SessionView::agents`), a
    /// line each, with the handle, the personality, and whether it takes part in this channel, in
    /// another of the session or in none. Only the issuer is answered.
    async fn execute(
        &self,
        _args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
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
}
