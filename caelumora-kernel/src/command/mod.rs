//! User commands (issue 28): components contribute the commands run on the channel
//! (`ChannelCommand`, issue 583), and a session's `CommandExecutor` finds and runs them on behalf
//! of a user. Components of a personality contribute the commands an agent carries out itself
//! (`caelumora_agents::command::AgentCommand`, issue 269), which its actor finds in a `CommandSet`
//! once the channel admitted the request, and the agent runs in a turn of its own.
//!
//! Commands are for people - agents act through tools. They are issued
//! through `UserActor::execute_command`, on behalf of the user the actor
//! stands for, and never enter the conversation as something said; what one
//! sends in their name - a call they made, `/workspace run` (issue 587) -
//! does, as theirs.
//!
//! The built-in command components live here too, one file per command, each its type. What every
//! command is made of - `Command`, `CommandProvider`, the `CommandSet`, the `CommandError` - is the
//! API's (`caelumora_api::command`, issues 516, 583).

pub(crate) mod agent;
pub(crate) mod channel;
pub mod help;
pub(crate) mod participant;
pub(crate) mod workspace;

pub use workspace::{RUN_COMMAND_NAME, SHELL_KIND};

use crate::actor::user::UserActor;
use crate::channel::ChannelLoop;
use crate::component::ComponentRegistry;
use crate::error::{refusal, user_message};
use crate::procedure::Procedure;
use crate::session::{SessionError, SessionLoop, SessionManager};
use crate::view::{ChannelView, ParticipantView, SessionView, Viewer, Views};
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{
    check_arguments, Command, CommandError, CommandProvider, CommandResult, CommandSet,
};
use caelumora_api::component::{ComponentImpl, ComponentResult};
use caelumora_domain::actor::SpanId;
use caelumora_domain::command::{CommandDescription, CommandOutput, CommandVisibility};
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreError;
use caelumora_domain::type_map::TypeKey;
use caelumora_domain::value::name::{named, Name};
use caelumora_security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use std::fmt::Debug;
use std::sync::Arc;

/// What issuing a command answered at once (see
/// `UserActor::execute_command`).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReply {
    /// The command's name, without the slash.
    pub command: String,
    /// What it answered: the answer of a command of the channel; for one
    /// sent to agents, which of them were not asked, if any were left out.
    pub output: Option<String>,
    /// `Channel` if the answer was also announced to the channel as a
    /// `CommandExecuted`. The issuer gets it here either way: the channel
    /// does not route an envelope back to its sender.
    pub visibility: CommandVisibility,
    /// For a command sent to agents (issue 269), the span of the requests:
    /// their answers come later, as `CommandExecuted` or `CommandFailed`
    /// on the issuer's stream, in this span.
    pub span_id: Option<SpanId>,
}

/// What a command of the channel runs with: who issued it, the channel,
/// and the session's other commands. Deliberately no `Turn` - such a
/// command runs outside of any agent's turn, often because none is
/// running.
pub struct CommandContext<'a> {
    /// The issuing user's actor in the channel: their security context, which
    /// whatever a command does is asked in (`UserActor::security_context`),
    /// whatever it read before; and what a command sends in their name it
    /// sends through it - an actor sends only in its own name (issue 228).
    pub issuer: &'a UserActor,
    pub channel: &'a ChannelLoop,
    pub executor: &'a CommandExecutor,
}

impl CommandContext<'_> {
    /// The running session the channel belongs to, for the command `command`: what a command acts
    /// on in the session, in the issuer's context (`issuer`).
    ///
    /// # Errors
    /// `CommandError::Failed` if the channel belongs to no session, which a channel a command
    /// runs in always does.
    pub(crate) fn session_loop(&self, command: &str) -> CommandResult<Arc<SessionLoop>> {
        self.channel
            .session_loop()
            .ok_or_else(|| command_error(command, &Error::msg("the channel belongs to no session")))
    }

    /// Carries out the procedure `make` builds from the session manager and the session the
    /// channel belongs to (issue 593) - a command is an entry point, which makes the procedures it
    /// asks for -, failing as its building block failed (`SessionError::untyped`).
    ///
    /// # Errors
    /// If the channel belongs to no session or the session manager is gone; as the procedure.
    pub(crate) async fn carry_out<P>(
        &self,
        make: impl FnOnce(&Arc<SessionManager>, SessionId) -> P,
    ) -> Result<P::Output, Error>
    where
        P: Procedure<Error = SessionError>,
    {
        let session = self
            .channel
            .session_loop()
            .ok_or_else(|| Error::msg("the channel belongs to no session"))?;
        let sessions =
            session.sessions().ok_or_else(|| Error::msg("the session manager is gone"))?;
        let procedure = make(&sessions, session.id());
        session.procedures().run(procedure).await.map_err(SessionError::untyped)
    }

    /// The views of the session the channel belongs to in the issuer's name - what the issuer may
    /// read there -, for the command `command`.
    ///
    /// # Errors
    /// As `session_loop`.
    pub(crate) fn views(&self, command: &str) -> CommandResult<Views> {
        Ok(self.session_loop(command)?.views(Viewer::new(self.issuer.security_context())))
    }

    /// The session the channel belongs to as the issuer reads it, for the command `command`: its
    /// view, reached through `views`.
    ///
    /// # Errors
    /// `CommandError::Failed` if the channel belongs to no session or the session cannot be read.
    pub(crate) async fn session_view(&self, command: &str) -> CommandResult<SessionView> {
        let views = self.views(command)?;
        views
            .session(self.channel.session_id())
            .await
            .map_err(|e| command_error(command, &e.into()))
    }

    /// The channel the command `command` runs in as the issuer reads it: its view, reached through
    /// `views`.
    ///
    /// # Errors
    /// `CommandError::Failed` if the channel belongs to no session, or the session or the channel
    /// cannot be read.
    pub(crate) async fn channel_view(&self, command: &str) -> CommandResult<ChannelView> {
        let views = self.views(command)?;
        views
            .channel(self.channel.session_id(), self.channel.id())
            .await
            .map_err(|e| command_error(command, &e.into()))
    }
}

/// A user command run on the channel (issue 583) - `/help`, `/agent wakeup` -: given the channel,
/// not an agent's turn. Its components contribute it through `CommandProvider<dyn
/// ChannelCommand>`, and a session's `CommandExecutor` runs it.
#[async_trait]
pub trait ChannelCommand: Command {
    /// Runs the command with `args`, the rest of the input line after its name, trimmed, which it
    /// parses itself; a command that names no parameters is never given any, one with a required
    /// parameter never none (`check_arguments`).
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` for arguments it cannot use,
    /// `CommandError::Forbidden` if the action it performs is refused, and
    /// `CommandError::Failed` otherwise.
    async fn execute(
        &self,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput>;
}

/// Finds and runs the commands of the channel - the session's set, run in
/// each of its channels - with the permission checks for seeing and running
/// them: `ShowCommand` and `ExecuteCommand`, with the attributes of the
/// channel they are run in (issue 304), as the `ToolExecutor` does for an
/// agent's tools. Like it, it is no component: how a command runs is the
/// command's own business, and there is nothing about running them to try
/// out. An agent's commands are admitted at the channel's door, with the
/// agent's attributes besides (issue 516).
#[derive(Debug)]
pub struct CommandExecutor {
    access_policy: Arc<dyn AccessPolicy>,
    commands: CommandSet<dyn ChannelCommand>,
}

impl CommandExecutor {
    /// The facets the commands of a session are built for (`Kernel::new_command_executor`):
    /// in the name of its owner, so that none is a library's (issue 516).
    pub const COMPONENT_FACETS: &[TypeKey] =
        &[TypeKey::of::<dyn CommandProvider<dyn ChannelCommand>>()];

    /// An executor for the commands `providers` contribute. Building them is
    /// up to whoever owns their configuration - for a session's commands,
    /// `Kernel::new_command_executor`.
    ///
    /// # Errors
    /// As `CommandSet::new`.
    pub fn new(
        access_policy: Arc<dyn AccessPolicy>,
        providers: Vec<ComponentImpl<dyn CommandProvider<dyn ChannelCommand>>>,
    ) -> ComponentResult<Self> {
        Ok(Self { access_policy, commands: CommandSet::new(providers)? })
    }

    /// An executor without commands.
    pub fn empty(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self { access_policy, commands: CommandSet::empty() }
    }

    /// The command of the session's that `command` collides with, whoever may see it
    /// (`CommandSet::colliding`): an agent's command must not be hidden by one of the session.
    pub(crate) fn colliding(&self, command: &CommandDescription) -> Option<&CommandDescription> {
        self.commands.colliding(command)
    }

    /// The commands `issuer` may see in the channel with `channel`
    /// attributes (`ShowCommand`), sorted by name.
    pub(crate) async fn visible(
        &self,
        issuer: &SecurityContext,
        channel: &SecurityAttributes,
    ) -> Vec<CommandDescription> {
        let mut visible = Vec::new();
        for command in self.commands.descriptions() {
            let show = ActionType::ShowCommand { command: &command, channel };
            if self.access_policy.check(issuer, show).await {
                visible.push(command);
            }
        }
        visible
    }

    /// The command `line` begins with - by its name or an alias - and the rest of the line, its
    /// arguments (`CommandSet::find`), whoever may see it: running it asks that.
    pub(crate) fn find<'a>(&self, line: &'a str) -> Option<(&dyn ChannelCommand, &'a str)> {
        self.commands.find(line).map(|(command, args)| (command.as_ref(), args))
    }

    /// Runs `command`, found by `find`, with `args` for `context.issuer`: `Unknown` if the issuer
    /// may not see it, since one they may not see is unknown to them, not refused; `Forbidden` if
    /// they may see but not run it; `InvalidArguments` if it takes no arguments and was given some,
    /// or requires one and was given none (`check_arguments`).
    ///
    /// # Errors
    /// As above; otherwise whatever the command reports.
    pub(crate) async fn execute(
        &self,
        command: &dyn ChannelCommand,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let description = command.description();
        let name = || description.name.to_string();
        let issuer = context.issuer.security_context();
        let channel = context.channel.security_attributes();
        let show = ActionType::ShowCommand { command: description, channel: &channel };
        self.access_policy.require(&issuer, show, |_| CommandError::Unknown(name())).await?;
        self.access_policy
            .require(
                &issuer,
                ActionType::ExecuteCommand { command: description, channel: &channel },
                |refused| CommandError::Forbidden { name: name(), refused },
            )
            .await?;
        check_arguments(description, args)?;
        command.execute(args, context).await
    }
}

/// The one of `among` - participants, agents, anything with a name, which `name` gives - that the
/// argument `wanted` of the command `command` names (`named`, issues 277, 278, 560): the one way
/// the commands of the channel that take someone find them and word that they did not - those
/// who take part `in this channel`, the agents `of this session`, as `place` says. What they are
/// sought among is the command's: `/agent wakeup` and `/participant nudge` act on an actor, and
/// seek among those running; `/participant remove` among those the issuer reads taking part,
/// `/channel leave` among the issuer's own agents of those, `/agent usage` among the agents of
/// those; `/agent delete` among the session's agents the issuer may read.
///
/// # Errors
/// `CommandError::InvalidArguments` naming the handles of `among`, if `wanted` names none, or
/// those it names, if it names several: nobody is picked.
pub(crate) fn named_one<T>(
    command: &str,
    wanted: &str,
    among: Vec<T>,
    name: impl Fn(&T) -> &Name,
    place: &str,
) -> CommandResult<T> {
    let invalid =
        |reason: String| CommandError::InvalidArguments { command: command.to_string(), reason };
    let mut handles: Vec<String> =
        among.iter().filter_map(|m| name(m).handle()).map(|handle| format!("@{handle}")).collect();
    handles.sort();
    let mut found = named(among, wanted, &name);
    match found.len() {
        1 => Ok(found.remove(0)),
        0 if handles.is_empty() => Err(invalid(format!("nobody '{wanted}' {place}"))),
        0 => {
            Err(invalid(format!("nobody '{wanted}' {place}; to choose from: {}", names(&handles))))
        }
        _ => {
            let several: Vec<String> = found.iter().map(|m| name(m).to_string()).collect();
            Err(invalid(format!("'{wanted}' names several {place}: {}", names(&several))))
        }
    }
}

/// Who takes part in the channel the command `command` runs in, as the issuer reads it
/// (`ChannelView::participants`): whom `/participant list` lists, and whom `/participant remove`,
/// `/channel leave` and `/agent usage` seek among.
///
/// # Errors
/// `CommandError::Failed` if the channel cannot be read.
pub(crate) async fn taking_part(
    command: &str,
    context: &CommandContext<'_>,
) -> CommandResult<Vec<ParticipantView>> {
    Ok(context.channel_view(command).await?.participants().await)
}

/// Of `participants`, those a command can name, each with its name (`named_one`): one that no
/// longer exists has no name to be named by.
pub(crate) fn named_participants(
    participants: Vec<ParticipantView>,
) -> Vec<(Name, ParticipantView)> {
    participants.into_iter().filter_map(|p| Some((p.name()?.clone(), p))).collect()
}

/// What `error` of an operation the command `command` ran means for its issuer: a refusal is the
/// command refused (`Forbidden`), a name another has taken an argument that does not fit
/// (`InvalidArguments`); anything else is a failure, logged, and described only as far as
/// `user_message` allows (`Failed`).
pub(crate) fn command_error(command: &str, error: &Error) -> CommandError {
    if let Some(refused) = refusal(error) {
        return CommandError::Forbidden { name: command.to_string(), refused: refused.clone() };
    }
    if let Some(taken @ StoreError::NameTaken { .. }) = error.downcast_ref() {
        return CommandError::InvalidArguments {
            command: command.to_string(),
            reason: taken.to_string(),
        };
    }
    tracing::warn!("/{} failed: {:#}", command, error);
    let reason = user_message(error).unwrap_or_else(|| "see the server log".to_string());
    CommandError::Failed { command: command.to_string(), reason }
}

/// The first word of `line`, what a command that was not found is told by.
pub(crate) fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or_default()
}

/// `names` joined as a sentence: "A, B and C".
pub(crate) fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
    }
}

/// Registers the command kinds with `registry`.
pub fn register_all(registry: &mut ComponentRegistry) {
    agent::register(registry);
    channel::register(registry);
    help::register(registry);
    participant::register(registry);
    workspace::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::component::ComponentFailPolicy;
    use caelumora_domain::value::command_name::CommandName;
    use caelumora_security::AllowAllPolicy;

    /// A command of the tests, named as it is told, which answers nothing.
    #[derive(Debug)]
    struct Named(CommandDescription);

    impl Command for Named {
        fn description(&self) -> &CommandDescription {
            &self.0
        }
    }

    #[async_trait]
    impl ChannelCommand for Named {
        async fn execute(
            &self,
            _args: &str,
            _context: &CommandContext<'_>,
        ) -> CommandResult<CommandOutput> {
            Ok(CommandOutput::private(""))
        }
    }

    fn provider(
        id: &str,
        command: &'static str,
    ) -> ComponentImpl<dyn CommandProvider<dyn ChannelCommand>> {
        let named: Vec<Arc<dyn ChannelCommand>> = vec![Arc::new(Named(CommandDescription {
            name: CommandName::fixed(command),
            aliases: Vec::new(),
            description: String::new(),
            parameters: Vec::new(),
            owners_only: false,
        }))];
        ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailOpen,
            facet: Arc::new(named),
        }
    }

    #[tokio::test]
    async fn two_components_with_the_same_command_are_a_configuration_error() {
        let access_policy = Arc::new(AllowAllPolicy::new());
        let error = CommandExecutor::new(
            Arc::<AllowAllPolicy>::clone(&access_policy),
            vec![provider("help", "help"), provider("my-help", "help")],
        )
        .expect_err("the second /help would be hidden by the first");
        let message = error.to_string();
        assert!(
            message.contains("/help")
                && message.contains("'help'")
                && message.contains("'my-help'"),
            "{message}"
        );

        let executor = CommandExecutor::new(
            access_policy,
            vec![provider("wakeup", "wakeup"), provider("help", "help")],
        )
        .unwrap();
        let names: Vec<String> = executor
            .visible(&SecurityContext::default(), &SecurityAttributes::systems())
            .await
            .into_iter()
            .map(|c| c.name.to_string())
            .collect();
        assert_eq!(names, vec!["help", "wakeup"], "sorted by name");
    }

    #[test]
    fn names_are_joined_as_a_sentence() {
        assert_eq!(names(&[]), "");
        assert_eq!(names(&["Sassy".to_string()]), "Sassy");
        assert_eq!(names(&["A".to_string(), "B".to_string(), "C".to_string()]), "A, B and C");
    }
}
