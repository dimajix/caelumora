//! The built-in `/agent create` command (issues 555, 560).

use crate::agent::{AgentManager, AgentSpec};
use crate::command::{command_error, CommandContext};
use anyhow::Error;
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_api::reader::ViewError;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::scope::Scope;
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

pub(super) const CREATE_COMMAND_NAME: &str = "agent create";

/// `/agent create <personality> [as <name>]`: makes an agent of the personality named.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(CREATE_COMMAND_NAME),
        aliases: Vec::new(),
        description:
            "Makes an agent of the personality named, in this session, named after it unless named."
                .to_string(),
        usage: Some("<personality> [as <name>]".to_string()),
        owners_only: false,
    }
}

/// Runs `/agent create` with `args`, making the agent with `agents`: of the personality named, as
/// the issuer finds it from the channel (`Views::personality_named`), in the session's scope - or
/// in the channel's, for a personality of the channel's scope, which makes agents only there
/// (`Scope::admits`) -, named after the personality unless `as` names it; the last `as` separates
/// the two, so that a personality's name may hold one (`AgentManager::create`). It does not bring
/// the agent in: `/participant add` does. Only the issuer is answered: nothing changed in the
/// channel.
pub(super) async fn execute(
    agents: &AgentManager,
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let invalid = |reason: String| CommandError::InvalidArguments {
        command: CREATE_COMMAND_NAME.to_string(),
        reason,
    };
    let (personality, name) = match args.rsplit_once(" as ") {
        Some((personality, name)) => (personality.trim(), name.trim()),
        None => (args.trim(), args.trim()),
    };
    if personality.is_empty() || name.is_empty() {
        return Err(invalid("name the personality, and the agent after 'as'".to_string()));
    }
    let unusable = |what: &str| invalid(format!("'{what}' is no name"));
    let personality = Name::new(personality).map_err(|_| unusable(personality))?;
    let name = Name::new(name).map_err(|_| unusable(name))?;
    let failed = |e: Error| command_error(CREATE_COMMAND_NAME, &e);
    let (session, views) = context.session_loop(CREATE_COMMAND_NAME)?;
    let place = Scope::ChannelScope { session_id: session.id(), channel_id: context.channel.id() };
    let personality = match views.personality_named(&personality, &place).await {
        Ok(personality) => personality,
        Err(e @ ViewError::NoPersonalityNamed(_)) => return Err(invalid(e.to_string())),
        Err(e) => return Err(failed(e.into())),
    };
    let scope = match &personality.name.scope {
        in_channel @ Scope::ChannelScope { .. } => in_channel.clone(),
        _ => Scope::SessionScope { session_id: session.id() },
    };
    let spec = AgentSpec::new(name.as_str(), scope, personality.id);
    let agent = agents.create(views.context(), spec).await.map_err(|e| failed(e.into()))?;
    Ok(CommandOutput::private(format!(
        "Made the agent {} of {}; /participant add brings it into a channel.",
        agent.name.name, personality.name.name
    )))
}
