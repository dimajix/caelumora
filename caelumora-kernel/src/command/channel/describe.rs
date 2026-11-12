//! The built-in `/channel describe` command (issue 560).

use crate::command::{command_error, CommandContext};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_api::reader::ViewError;
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
/// session `args` names as the issuer may read it - by its whole name, which is the only one of
/// the session (`SessionView::channel_named`), a channel they may not read named as one that does
/// not exist - with its summary, briefing, classification and language, its own or else its
/// session's. Only the issuer is answered.
pub(super) async fn execute(
    args: &str,
    context: &CommandContext<'_>,
) -> CommandResult<CommandOutput> {
    let wanted = args.trim();
    let channel = match wanted {
        "" => context.channel_view(DESCRIBE_COMMAND_NAME).await?,
        _ => {
            let session = context.session_view(DESCRIBE_COMMAND_NAME).await?;
            match session.channel_named(wanted).await {
                Ok(channel) => channel,
                Err(e @ ViewError::NoChannelNamed(_)) => {
                    return Err(CommandError::InvalidArguments {
                        command: DESCRIBE_COMMAND_NAME.to_string(),
                        reason: e.to_string(),
                    })
                }
                Err(e) => return Err(command_error(DESCRIBE_COMMAND_NAME, &e.into())),
            }
        }
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
