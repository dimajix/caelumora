//! The built-in `/channel leave` command (issue 562).

use crate::command::{command_error, named_one, CommandContext};
use caelumora_api::actor::Actor;
use caelumora_api::command::CommandResult;
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

pub(super) const LEAVE_COMMAND_NAME: &str = "channel leave";

/// `/channel leave [agent]`: takes the issuer, or an agent of theirs, out of this channel.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(LEAVE_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Leaves this channel, or takes the agent of yours named out of it; you stay \
                      in the session."
            .to_string(),
        usage: Some("[agent]".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel leave` with `args`: the issuer leaves the channel the command was issued in, or
/// with a name takes the agent of theirs it names out of it - among the agents taking part they may
/// take out (`ParticipantView::may_leave`), found by name or handle (`named_one`) -, by their own
/// act (`ChannelLoop::leave`, which asks `LeaveChannel`). Neither leaves the session. The command
/// runs in the issuer's call, so its answer still reaches them once they left; only they are
/// answered: the channel tells everyone that someone left.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let wanted = args.trim();
    let name = context.channel.name();
    let (leaving, done) = if wanted.is_empty() {
        (context.issuer.id(), format!("You left {name}."))
    } else {
        // One that no longer exists has no name to be named by.
        let agents: Vec<(Name, ActorId)> = context
            .channel_view(LEAVE_COMMAND_NAME)
            .await?
            .participants()
            .await
            .into_iter()
            .filter(|p| p.id().as_agent_id().is_some() && p.may_leave())
            .filter_map(|p| Some((Name::new(p.name()?).ok()?, p.id())))
            .collect();
        let (agent, id) = named_one(
            LEAVE_COMMAND_NAME,
            wanted,
            agents,
            |(name, _)| name,
            "of yours in this channel",
        )?;
        (id, format!("{agent} left {name}."))
    };
    context
        .channel
        .leave(&context.issuer.security_context(), leaving)
        .await
        .map_err(|e| command_error(LEAVE_COMMAND_NAME, &e))?;
    Ok(CommandOutput::private(done))
}
