//! The built-in command group `agent` (issue 560): the commands on the agents of a session -
//! `agent list`, `create`, `delete`, `wakeup`, `cancel` and `usage` -, contributed by one
//! component, one file per command. The commands an agent carries out itself are its own
//! (`caelumora_agents::command`).

mod cancel;
mod create;
mod delete;
mod list;
mod usage;
mod wakeup;

use crate::agent::AgentManager;
use crate::command::{command_error, ChannelCommand, CommandContext};
use crate::component::ComponentRegistry;
use crate::view::{AgentView, SessionView};
use caelumora_api::command::{CommandProvider, CommandResult};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::AGENT_GROUP_KIND;
use std::sync::Arc;

/// The commands of the group `agent`: `/agent list`, `/agent create`, `/agent delete`,
/// `/agent wakeup` (`/wakeup`), `/agent cancel` (`/cancel`) and `/agent usage` - `/agent create`
/// making agents with `agents`, `/agent delete` deleting them with it.
pub(crate) fn commands(agents: &Arc<AgentManager>) -> Vec<Arc<dyn ChannelCommand>> {
    vec![
        Arc::new(list::ListCommand::new()),
        Arc::new(create::CreateCommand::new(Arc::clone(agents))),
        Arc::new(delete::DeleteCommand::new(Arc::clone(agents))),
        Arc::new(wakeup::WakeupCommand::new()),
        Arc::new(cancel::CancelCommand::new()),
        Arc::new(usage::UsageCommand::new()),
    ]
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
            .provides::<dyn CommandProvider<dyn ChannelCommand>>()
            .uses::<AgentManager>(),
        |_descriptor, context| {
            let agents = Arc::clone(context.service::<AgentManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(commands(&agents)));
            Ok(facets)
        },
    );
}
