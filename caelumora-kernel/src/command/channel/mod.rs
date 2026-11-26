//! The built-in command group `channel` (issue 560): the commands on the channels of a session -
//! `channel list`, `describe`, `create`, `delete`, `join`, `leave`, `rename`, `summary` and
//! `briefing` -, contributed by one component, one file per command; `channel notes` is the notes
//! plugin's (issue 607). Those that change or
//! leave a channel act on the one they are issued in; those that make, delete or join one name it.

mod briefing;
mod create;
mod delete;
mod describe;
mod join;
mod leave;
mod list;
mod rename;
mod summary;

use crate::command::ChannelCommand;
use crate::component::ComponentRegistry;
use caelumora_api::command::CommandProvider;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::command::CHANNEL_GROUP_KIND;
use std::sync::Arc;

/// The commands of the group `channel`: `/channel list`, `/channel describe`, `/channel create`,
/// `/channel delete`, `/channel join`, `/channel leave`, `/channel rename`, `/channel summary` and
/// `/channel briefing`.
fn commands() -> Vec<Arc<dyn ChannelCommand>> {
    vec![
        Arc::new(list::ListCommand::new()),
        Arc::new(describe::DescribeCommand::new()),
        Arc::new(create::CreateCommand::new()),
        Arc::new(delete::DeleteCommand::new()),
        Arc::new(join::JoinCommand::new()),
        Arc::new(leave::LeaveCommand::new()),
        Arc::new(rename::RenameCommand::new()),
        Arc::new(summary::SummaryCommand::new()),
        Arc::new(briefing::BriefingCommand::new()),
    ]
}

/// Registers the kind `channel` with `registry`.
pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_GROUP_KIND)
            .provides::<dyn CommandProvider<dyn ChannelCommand>>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CommandProvider<dyn ChannelCommand>>(Arc::new(commands()));
            Ok(facets)
        },
    );
}
