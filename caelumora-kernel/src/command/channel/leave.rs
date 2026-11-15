//! The built-in `/channel leave` command (issue 562).

use crate::command::{
    command_error, named_one, named_participants, taking_part, ChannelCommand, CommandContext,
};
use async_trait::async_trait;
use caelumora_api::actor::Actor;
use caelumora_api::command::{Command, CommandResult};
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::value::command_name::CommandName;

const LEAVE_COMMAND_NAME: &str = "channel leave";

/// `/channel leave [agent]`: takes the issuer, or an agent of theirs, out of this channel.
#[derive(Debug)]
pub(super) struct LeaveCommand(CommandDescription);

impl LeaveCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(LEAVE_COMMAND_NAME),
            aliases: Vec::new(),
            description:
                "Leaves this channel, or takes the agent of yours named out of it; you stay \
                          in the session."
                    .to_string(),
            parameters: vec![CommandParameter::optional("agent").of_kind(ParameterKind::Agent)],
            owners_only: false,
        })
    }
}

impl Command for LeaveCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl ChannelCommand for LeaveCommand {
    /// Runs `/channel leave` with `args`: the issuer leaves the channel the command was issued in,
    /// or with a name takes the agent of theirs it names out of it - among the agents taking part
    /// they may take out (`ParticipantView::may_leave`), found by name or handle (`named_one`) -,
    /// by their own act (`ChannelLoop::leave`, which asks `LeaveChannel`). Neither leaves the
    /// session. The command runs in the issuer's call, so its answer still reaches them once they
    /// left; only they are answered: the channel tells everyone that someone left.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let wanted = args.trim();
        let name = context.channel.name();
        let (leaving, done) = if wanted.is_empty() {
            (context.issuer.id(), format!("You left {name}."))
        } else {
            let mut participants = taking_part(LEAVE_COMMAND_NAME, context).await?;
            participants.retain(|p| p.id().as_agent_id().is_some() && p.may_leave());
            let (agent, participant) = named_one(
                LEAVE_COMMAND_NAME,
                wanted,
                named_participants(participants),
                |(name, _)| name,
                "of yours in this channel",
            )?;
            (participant.id(), format!("{agent} left {name}."))
        };
        context
            .channel
            .leave(&context.issuer.security_context(), leaving)
            .await
            .map_err(|e| command_error(LEAVE_COMMAND_NAME, &e))?;
        Ok(CommandOutput::private(done))
    }
}
