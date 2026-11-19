//! The built-in `/channel join` command (issue 560).

use crate::channel::ChannelError;
use crate::command::{command_error, ChannelCommand, CommandContext};
use crate::error::is_forbidden;
use crate::procedure::participant::TakeIn;
use crate::session::SessionError;
use async_trait::async_trait;
use caelumora_api::actor::Actor;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::store::StoreError;
use caelumora_domain::value::command_name::CommandName;

const JOIN_COMMAND_NAME: &str = "channel join";

/// `/channel join <channel>`: takes the issuer into a channel of the session, by their own act.
#[derive(Debug)]
pub(super) struct JoinCommand(CommandDescription);

impl JoinCommand {
    /// The command, as `/help` tells it.
    pub(super) fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(JOIN_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Joins the channel of this session named, if you may join it.".to_string(),
            parameters: vec![CommandParameter::required("channel").of_kind(ParameterKind::Channel)],
            owners_only: false,
        })
    }
}

impl Command for JoinCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

/// Whether `error` of joining a channel named by name would tell more than that the issuer may not
/// join it: not cleared, out of scope, not found as the issuer is read, or the channel gone or
/// stopped since it was found.
fn hides_the_channel(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref(),
        Some(ChannelError::NotCleared { .. } | ChannelError::OutOfScope { .. })
    ) || matches!(error.downcast_ref(), Some(StoreError::NotFound { .. }))
        || matches!(
            error.downcast_ref(),
            Some(SessionError::ChannelNotFound { .. } | SessionError::ChannelUnavailable { .. })
        )
}

#[async_trait]
impl ChannelCommand for JoinCommand {
    /// Runs `/channel join` with `args`: the issuer joins the running channel of the session of
    /// that whole name (`SessionLoop::channel_named`), through the session, which makes one change
    /// of a participant at a time (`TakeIn::joining`, issues 590, 593), and asks `JoinChannel` in
    /// their own context. Who may join a channel reads nothing of it before they did, so it is not
    /// sought among those they read; and a channel they may not join - by its access list, or not
    /// cleared for it -, or that does not run, is named as one that does not exist, so that
    /// channels cannot be probed. Only the issuer is answered: the channel tells everyone that they
    /// joined.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: JOIN_COMMAND_NAME.to_string(),
            reason,
        };
        let wanted = args.trim();
        let nowhere = || invalid(format!("no channel '{wanted}' you may join"));
        let session = context.session_loop(JOIN_COMMAND_NAME)?;
        let channel = session.channel_named(wanted).ok_or_else(nowhere)?;
        let (issuer, name) = (context.issuer.id(), channel.name());
        if channel.channel().lists(issuer) {
            return Ok(CommandOutput::private(format!("You take part in {name} already.")));
        }
        // Through the session, which makes one change of a participant at a time (issue 590).
        let (context_of_issuer, into) = (context.issuer.security_context(), Some(channel.id()));
        let joining = context.carry_out(|sessions, session| {
            TakeIn::joining(sessions, &context_of_issuer, session, issuer, into)
        });
        match joining.await {
            Ok(_) => Ok(CommandOutput::private(format!("Joined {name}."))),
            // Refused - the channel's entry, the session's for one no longer in it, a store's
            // check -, not cleared, out of scope, or gone or stopped meanwhile: named alike, so
            // that channels cannot be probed.
            Err(e) if is_forbidden(&e) || hides_the_channel(&e) => Err(nowhere()),
            Err(e) => Err(command_error(JOIN_COMMAND_NAME, &e)),
        }
    }
}
