//! The built-in command group `agent` (issue 560): the commands on the agents of a session -
//! `agent list`, `create`, `delete` and `wakeup` -, contributed by one component, one file per
//! command. The commands an agent carries out itself are its own (`caelumora_agents::command`).

mod create;
mod delete;
mod list;
mod wakeup;

use crate::agent::AgentManager;
use crate::command::{command_error, ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use crate::view::{AgentView, SessionView};
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::{Command, CommandOutput, AGENT_GROUP_KIND};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// The component of the command group `agent`: `/agent list`, `/agent create`, `/agent delete` and
/// `/agent wakeup` (`/wakeup`). It holds the `AgentManager`, which `/agent create` makes agents
/// with and `/agent delete` deletes them with.
#[derive(Debug)]
pub(crate) struct AgentGroupComponent {
    agents: Arc<AgentManager>,
}

impl AgentGroupComponent {
    /// The component, making agents with `agents`.
    pub(crate) fn new(agents: Arc<AgentManager>) -> Self {
        Self { agents }
    }
}

impl ProvidesCommands for AgentGroupComponent {
    fn commands(&self) -> Vec<Command> {
        vec![list::command(), create::command(), delete::command(), wakeup::command()]
    }
}

#[async_trait]
impl ChannelCommandProvider for AgentGroupComponent {
    async fn execute(
        &self,
        name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        match name.as_str() {
            list::LIST_COMMAND_NAME => list::execute(args, context).await,
            create::CREATE_COMMAND_NAME => create::execute(&self.agents, args, context).await,
            delete::DELETE_COMMAND_NAME => delete::execute(&self.agents, args, context).await,
            wakeup::WAKEUP_COMMAND_NAME => wakeup::execute(args, context).await,
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

/// The session the channel the command `command` runs in belongs to, as the issuer reads it, with
/// its agents (`SessionView::agents`): whom `/agent list` lists and `/agent delete` deletes.
///
/// # Errors
/// `CommandError::Failed` if the session or its agents cannot be read.
async fn session_agents(
    command: &str,
    context: &CommandContext<'_>,
) -> CommandResult<(SessionView, Vec<AgentView>)> {
    let session = context.session_view(command).await?;
    let agents = session.agents().await.map_err(|e| command_error(command, &e.into()))?;
    Ok((session, agents))
}

/// Registers the kind `agent` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(AGENT_GROUP_KIND)
            .provides::<dyn ChannelCommandProvider>()
            .uses::<AgentManager>(),
        |_descriptor, context| {
            let agents = Arc::clone(context.service::<AgentManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(AgentGroupComponent::new(agents)));
            Ok(facets)
        },
    );
}
