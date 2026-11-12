//! The built-in `/channel describe` command (issue 560).

use super::channel_named;
use crate::command::CommandContext;
use caelumora_api::command::CommandResult;
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::value::command_name::CommandName;

pub(super) const DESCRIBE_COMMAND_NAME: &str = "channel describe";

/// `/channel describe [channel]`: what a channel is and is for.
pub(super) fn command() -> Command {
    Command {
        name: CommandName::fixed(DESCRIBE_COMMAND_NAME),
        aliases: Vec::new(),
        description: "Describes this channel, or the one of this session named.".to_string(),
        usage: Some("[channel]".to_string()),
        owners_only: false,
    }
}

/// Runs `/channel describe` with `args`: the channel the command was issued in, or the one of the
/// session `args` names (`channel_named`) - with its summary, briefing, classification and
/// language, its own or else its session's. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let wanted = args.trim();
    let channel = match wanted {
        "" => context.channel_view(DESCRIBE_COMMAND_NAME).await?,
        _ => channel_named(DESCRIBE_COMMAND_NAME, wanted, context).await?,
    };
    let said = |text: Option<&str>| text.unwrap_or("nothing said").to_string();
    let language = match (channel.language(), channel.effective_language()) {
        (Some(own), _) => own.as_str().to_string(),
        (None, Some(session)) => format!("{}, the session's", session.as_str()),
        (None, None) => "none in particular".to_string(),
    };
    Ok(CommandOutput::private(format!(
        "Channel {}\n\nSummary: {}\nBriefing: {}\nClassification: {}\nLanguage: {}",
        channel.name(),
        said(channel.summary().map(|s| s.as_str())),
        said(channel.briefing().map(|b| b.as_str())),
        channel.classification(),
        language
    )))
}
