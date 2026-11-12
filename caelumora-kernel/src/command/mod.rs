//! User commands (issue 28): `ChannelCommandProvider` components contribute
//! commands, and a session's `CommandExecutor` finds and runs them on behalf
//! of a user. `AgentCommandProvider` components of a personality contribute
//! the commands an agent carries out itself (issue 269), a `CommandSet` its
//! actor finds them in once the channel admitted the request, and the agent
//! runs in a turn of its own.
//!
//! Commands are for people - agents act through tools. They are issued
//! through `UserActor::execute_command`, on behalf of the user the actor
//! stands for, and never enter the conversation.
//!
//! The built-in command components live here too, one file per command. What an agent's commands
//! are made of - `ProvidesCommands`, the `CommandSet`, the `CommandError` - is the API's
//! (`caelumora_api::command`, issue 516).

pub(crate) mod agent;
pub(crate) mod channel;
pub mod help;
pub(crate) mod participant;

use crate::actor::user::UserActor;
use crate::channel::ChannelLoop;
use crate::component::ComponentRegistry;
use crate::error::{refusal, user_message};
use crate::session::SessionLoop;
use crate::view::{ChannelView, SessionView, Viewer, Views};
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, CommandSet, ProvidesCommands};
use caelumora_api::component::{ComponentImpl, ComponentResult};
use caelumora_domain::command::{Command, CommandOutput, CommandVisibility};
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::store::StoreError;
use caelumora_domain::type_map::TypeKey;
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::{named, Name};
use caelumora_security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use std::fmt::Debug;
use std::sync::Arc;
use uuid::Uuid;

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
    pub span_id: Option<Uuid>,
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
    /// `session_view`.
    ///
    /// # Errors
    /// `CommandError::Failed` if the session or the channel cannot be read.
    pub(crate) async fn channel_view(&self, command: &str) -> CommandResult<ChannelView> {
        let session = self.session_view(command).await?;
        session.channel(self.channel.id()).await.map_err(|e| command_error(command, &e.into()))
    }
}

/// A component facet contributing user commands run on the channel -
/// `/help`, `/wakeup` -, as `ToolProvider` contributes tools: given the
/// channel, not an agent's turn.
#[async_trait]
pub trait ChannelCommandProvider: ProvidesCommands {
    /// Runs the command `name` - by its name, whichever it was issued by - with `args`, the rest of
    /// the input line after it, which the command parses itself.
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` for arguments it cannot use,
    /// `CommandError::Forbidden` if the action it performs is refused, and
    /// `CommandError::Failed` otherwise.
    async fn execute(
        &self,
        name: &CommandName,
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
    commands: CommandSet<dyn ChannelCommandProvider>,
}

impl CommandExecutor {
    /// The facets the commands of a session are built for (`Kernel::new_command_executor`):
    /// in the name of its owner, so that none is a library's (issue 516).
    pub const COMPONENT_FACETS: &[TypeKey] = &[TypeKey::of::<dyn ChannelCommandProvider>()];

    /// An executor for the commands `providers` contribute. Building them is
    /// up to whoever owns their configuration - for a session's commands,
    /// `Kernel::new_command_executor`.
    ///
    /// # Errors
    /// As `CommandSet::new`.
    pub fn new(
        access_policy: Arc<dyn AccessPolicy>,
        providers: Vec<ComponentImpl<dyn ChannelCommandProvider>>,
    ) -> ComponentResult<Self> {
        Ok(Self { access_policy, commands: CommandSet::new(providers)? })
    }

    /// An executor without commands.
    pub fn empty(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self { access_policy, commands: CommandSet::empty() }
    }

    /// The command of the session's that `command` collides with, whoever may see it
    /// (`CommandSet::colliding`): an agent's command must not be hidden by one of the session.
    pub fn colliding(&self, command: &Command) -> Option<&Command> {
        self.commands.colliding(command)
    }

    /// The commands `issuer` may see in the channel with `channel`
    /// attributes (`ShowCommand`), sorted by name.
    pub async fn visible(
        &self,
        issuer: &SecurityContext,
        channel: &SecurityAttributes,
    ) -> Vec<Command> {
        let mut visible = Vec::new();
        for command in self.commands.commands() {
            let show = ActionType::ShowCommand { command: &command, channel };
            if self.access_policy.check(issuer, show).await {
                visible.push(command);
            }
        }
        visible
    }

    /// The name of the command `line` begins with - by its name or an alias - and the rest of
    /// the line, its arguments (`CommandSet::find`), whoever may see it: running it asks that.
    pub fn find<'a>(&self, line: &'a str) -> Option<(&CommandName, &'a str)> {
        self.commands.find(line).map(|(entry, args)| (&entry.command().name, args))
    }

    /// Runs the command `name` with `args` for `context.issuer`: `Unknown` if there is no such
    /// command or the issuer may not see it, since one they may not see is unknown to them, not
    /// refused; `Forbidden` if they may see but not run it.
    ///
    /// # Errors
    /// As above; otherwise whatever the command reports.
    pub async fn execute(
        &self,
        name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let unknown = || CommandError::Unknown(name.to_string());
        let entry = self.commands.entry(name.as_str()).ok_or_else(unknown)?;
        let issuer = context.issuer.security_context();
        let channel = context.channel.security_attributes();
        let command = entry.command();
        self.access_policy
            .require(&issuer, ActionType::ShowCommand { command, channel: &channel }, |_| unknown())
            .await?;
        self.access_policy
            .require(
                &issuer,
                ActionType::ExecuteCommand { command, channel: &channel },
                |refused| CommandError::Forbidden { name: name.to_string(), refused },
            )
            .await?;
        entry.provider().execute(name, args, context).await
    }
}

/// The one of `among` - participants, agents, anything with a name, which `name` gives - that the
/// argument `wanted` of the command `command` names (`named`, issues 277, 278, 560): the one way
/// the commands of the channel that take someone find them and word that they did not - those
/// who take part `in this channel`, the agents `of this session`, as `place` says. What they are
/// sought among is the command's: `/agent wakeup` and `/participant nudge` act on an actor, and
/// seek among those running; `/participant remove` among those the issuer reads taking part,
/// `/channel leave` among the issuer's own agents of those; `/agent delete` among the session's
/// agents the issuer may read.
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
    let mut handles: Vec<String> = among
        .iter()
        .map(|m| name(m).handle())
        .filter(|handle| !handle.is_empty())
        .map(|handle| format!("@{handle}"))
        .collect();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::component::ComponentFailPolicy;
    use caelumora_security::AllowAllPolicy;

    #[derive(Debug)]
    struct Named(&'static str);

    impl ProvidesCommands for Named {
        fn commands(&self) -> Vec<Command> {
            vec![Command {
                name: CommandName::fixed(self.0),
                aliases: Vec::new(),
                description: String::new(),
                usage: None,
                owners_only: false,
            }]
        }
    }

    #[async_trait]
    impl ChannelCommandProvider for Named {
        async fn execute(
            &self,
            _name: &CommandName,
            _args: &str,
            _context: &CommandContext<'_>,
        ) -> CommandResult<CommandOutput> {
            Ok(CommandOutput::private(""))
        }
    }

    fn provider(id: &str, command: &'static str) -> ComponentImpl<dyn ChannelCommandProvider> {
        ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailOpen,
            facet: Arc::new(Named(command)),
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
