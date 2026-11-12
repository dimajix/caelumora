//! The built-in `/participant add` command (issues 555, 560), `/add` for short.

use crate::command::{command_error, names, CommandContext};
use anyhow::Error;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::HistoryAccess;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::{Name, NameMatch};

pub(super) const ADD_COMMAND_NAME: &str = "participant add";

/// `/participant add <name>`: brings the user or the agent named into the channel, and into its
/// session first if not in it yet (`SessionLoop::add_participant`), sharing the whole history.
/// `/add` for short.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(ADD_COMMAND_NAME),
        aliases: vec![CommandName::fixed("add")],
        description: "Brings the user or agent named into this channel, and into the session if not in it yet; they see the whole history.".to_string(),
        usage: Some("<name>".to_string()),
        owners_only: false,
    }
}

/// Runs `/participant add` with `args`. The name is found from the channel
/// (`SessionView::candidates` by the whole name): of several of one name the nearest; several
/// equally near are refused with their names. Only the issuer is answered: the channel tells
/// everyone that someone joined.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: ADD_COMMAND_NAME.to_string(),
        reason,
    };
    let wanted = args.trim();
    if wanted.is_empty() {
        return Err(invalid("name whom to add".to_string()));
    }
    let nobody = || invalid(format!("nobody '{wanted}' to add"));
    let name = Name::new(wanted).map_err(|_| nobody())?;
    let failed = |e: Error| command_error(ADD_COMMAND_NAME, &e);
    let (session, views) = context.session_loop(ADD_COMMAND_NAME)?;
    let channel_id = context.channel.id();
    let found = views
        .session(session.id())
        .await
        .map_err(|e| failed(e.into()))?
        .candidates(&NameMatch::Exact(name), Some(channel_id))
        .await
        .map_err(|e| failed(e.into()))?;
    match found.as_slice() {
        [] => Err(nobody()),
        [one] if one.in_channel => {
            Ok(CommandOutput::private(format!("{} takes part already.", one.name)))
        }
        [one] => {
            session
                .add_participant(views.context(), one.id, Some((channel_id, HistoryAccess::All)))
                .await
                .map_err(failed)?;
            Ok(CommandOutput::private(format!("Added {}.", one.name)))
        }
        several => {
            let several: Vec<String> = several
                .iter()
                .map(|candidate| match candidate.id {
                    ActorId::User(_) => format!("the user {}", candidate.name),
                    _ => format!("the agent {}", candidate.name),
                })
                .collect();
            Err(invalid(format!(
                "'{}' names {}, who are equally near; add them in the web UI",
                wanted,
                names(&several)
            )))
        }
    }
}
