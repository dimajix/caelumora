//! The built-in `/channel describe` command (issue 560).

use super::this_or_named;
use crate::command::{ChannelCommand, CommandContext};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandParameter};
use caelumora_domain::value::command_name::CommandName;

const DESCRIBE_COMMAND_NAME: &str = "channel describe";

/// `/channel describe [channel]`: what a channel is and is for.
#[derive(Debug)]
pub(super) struct DescribeCommand(CommandDescription);

impl DescribeCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(DESCRIBE_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Describes this channel, or the one of this session named.".to_string(),
            parameters: vec![CommandParameter::optional("channel")],
            owners_only: false,
        })
    }
}

impl Command for DescribeCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for DescribeCommand {
    /// Runs `/channel describe` with `args`: the channel the command was issued in, or the one of
    /// the session `args` names (`this_or_named`) - with its summary, briefing, classification and
    /// language, its own or else its session's. Only the issuer is answered.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let channel = this_or_named(DESCRIBE_COMMAND_NAME, args, context).await?;
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
}
