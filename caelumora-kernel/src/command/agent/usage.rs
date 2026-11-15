//! The built-in `/agent usage` command (issue 573): how full the contexts of the channel's agents
//! are, and what their calls used.

use crate::command::{
    command_error, named_one, named_participants, taking_part, ChannelCommand, CommandContext,
};
use crate::view::ParticipantView;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::message::TokenUsage;
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;

const USAGE_COMMAND_NAME: &str = "agent usage";

/// `/agent usage [agent]`: how full the contexts of the agents of the channel are, or of one of
/// them, and what their calls used.
#[derive(Debug)]
pub(super) struct UsageCommand(CommandDescription);

impl UsageCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(USAGE_COMMAND_NAME),
            aliases: Vec::new(),
            description:
                "Tells how full the context of each agent of this channel is, or of the one \
                          named, and how many tokens its calls used."
                    .to_string(),
            usage: Some("[agent]".to_string()),
            owners_only: false,
        })
    }
}

impl Command for UsageCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for UsageCommand {
    /// Runs `/agent usage` with `args`: of the agents taking part in the channel as the issuer
    /// reads it (`taking_part`) and that can be named, a line each - or of the one named, found by
    /// name or handle (`named_one`) - with what its state holds of its calls
    /// (`ParticipantView::usage`); without an argument, a line more with the totals of them all, as
    /// the web UI adds them up above its list. Only the issuer is answered.
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` if the argument names no agent taking part, or several;
    /// `CommandError::Failed` if the channel or an agent's state cannot be read.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let failed = |e: Error| command_error(USAGE_COMMAND_NAME, &e);
        let mut participants = taking_part(USAGE_COMMAND_NAME, context).await?;
        participants.retain(|p| p.id().as_agent_id().is_some());
        let agents = named_participants(participants);
        let wanted = args.trim();
        if !wanted.is_empty() {
            let (name, agent) = named_one(
                USAGE_COMMAND_NAME,
                wanted,
                agents,
                |(name, _)| name,
                "among the agents in this channel",
            )?;
            let usage = agent.usage().await.map_err(|e| failed(e.into()))?;
            return Ok(CommandOutput::private(used_by(&name, &agent, usage)));
        }
        if agents.is_empty() {
            return Ok(CommandOutput::private("No agent takes part in this channel.".to_string()));
        }
        let mut text = String::from("What the agents of this channel used:\n\n");
        let (mut calls, mut input, mut output) = (0, 0, 0);
        for (name, agent) in &agents {
            let usage = agent.usage().await.map_err(|e| failed(e.into()))?;
            if let Some(usage) = usage {
                calls += usage.calls;
                input += usage.prompt_tokens;
                output += usage.completion_tokens;
            }
            text.push_str(&format!("- {}\n", used_by(name, agent, usage)));
        }
        text.push_str(&format!("\nIn all: {}.", calls_used(calls, input, output)));
        Ok(CommandOutput::private(text))
    }
}

/// The line of the agent `agent`, called `name`, that used `usage`: its context, against its size
/// where known, and the totals of its calls apart from it, since they grow much faster.
fn used_by(name: &Name, agent: &ParticipantView, usage: Option<TokenUsage>) -> String {
    let handle = agent.handle().map(|h| format!(" (@{h})")).unwrap_or_default();
    let Some(usage) = usage else { return format!("{name}{handle}: no calls counted") };
    let context = match (usage.context_size, usage.filled()) {
        (Some(size), Some(filled)) => {
            format!(
                "{} of {size} tokens in its context ({:.0} %)",
                usage.context_tokens,
                filled * 100.0
            )
        }
        _ => format!("{} tokens in its context, its size unknown", usage.context_tokens),
    };
    let calls = calls_used(usage.calls, usage.prompt_tokens, usage.completion_tokens);
    format!("{name}{handle}: {context}; {calls}")
}

/// `calls` calls that used `input` and `output` tokens, in words.
fn calls_used(calls: u64, input: u64, output: u64) -> String {
    let noun = if calls == 1 { "call" } else { "calls" };
    format!("{calls} {noun} used {input} input and {output} output tokens")
}
