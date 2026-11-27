//! The command of `subagents` (issue 596): **`/subagent @agent <personality> <brief>`**, with which
//! an agent's owner has it start a sub-agent of one of the personalities the component lists, as
//! its tool `start_subagent` does. The agent is given to read, in the owner's name, what it
//! started: the personality, the brief and the thread, as `/skill` gives a skill. The thread hangs
//! at that message, in the thread the command was issued in, where what the sub-agent answers is
//! given back to the agent and it responds.

use super::{listed, ASK_SUBAGENT, END_SUBAGENT};
use crate::extension::{ErrorClass, SubAgentError, SubAgentWriter};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::turn::Turn;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::command::{
    CommandDescription, CommandOutput, CommandParameter, ParameterKind,
};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::prompt_catalogue::{context, PromptTexts};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;
use tracing::warn;

/// The command that starts a sub-agent.
pub const SUBAGENT_COMMAND_NAME: &str = "subagent";

/// `/subagent @agent <personality> <brief>`: the agent starts a sub-agent on its owner's word.
#[derive(Debug)]
pub(super) struct SubagentCommand {
    description: CommandDescription,
    /// The personalities it may start an agent of.
    personalities: Vec<Name>,
    /// The texts of what the agent is given.
    prompts: PromptTexts,
}

impl SubagentCommand {
    /// The command, starting agents of `personalities`, its texts from `prompts`.
    pub(super) fn new(personalities: Vec<Name>, prompts: PromptTexts) -> Self {
        let description = CommandDescription {
            name: CommandName::fixed(SUBAGENT_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Has the agent start a sub-agent of a personality on a brief: it works on it apart, in a thread of its own, and gives back to the agent what came of it.".to_string(),
            parameters: vec![
                CommandParameter::required("personality").of_kind(ParameterKind::Personality),
                CommandParameter::required("brief"),
            ],
            // Like approving its calls: whoever answers for the agent decides what it starts.
            owners_only: true,
        };
        Self { description, personalities, prompts }
    }

    /// The personality `args` begin with, the one taking most of them where several do, and the
    /// brief after it.
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` if `args` begin with none of the personalities, or no
    /// brief follows.
    fn read<'a>(&self, args: &'a str) -> CommandResult<(&Name, &'a str)> {
        let begun = self.personalities.iter().filter_map(|p| Some((p, p.strip_from(args)?)));
        let (personality, brief) = begun
            .min_by_key(|(_, brief)| brief.len())
            .ok_or_else(|| invalid(format!("begin with one of {}", listed(&self.personalities))))?;
        if brief.is_empty() {
            return Err(invalid(format!("a brief for the sub-agent of '{personality}'")));
        }
        Ok((personality, brief))
    }

    /// What the agent is given to read of the sub-agent of `personality` it started on `brief` in
    /// `thread`.
    ///
    /// # Errors
    /// `CommandError::Failed` if the text cannot be had.
    fn started(&self, personality: &Name, brief: &str, thread: ThreadId) -> CommandResult<String> {
        let values = context! {
            personality => personality.as_str(),
            brief,
            thread => thread.to_string(),
            ask => ASK_SUBAGENT,
            end => END_SUBAGENT,
        };
        let started = self.prompts.render(&format!("{NAMESPACE}.started"), values);
        started.map_err(|error| failed(error.to_string()))
    }
}

/// The command's arguments refused for `reason`.
fn invalid(reason: String) -> CommandError {
    CommandError::InvalidArguments { command: SUBAGENT_COMMAND_NAME.to_string(), reason }
}

/// The command failed for `reason`.
fn failed(reason: String) -> CommandError {
    CommandError::Failed { command: SUBAGENT_COMMAND_NAME.to_string(), reason }
}

/// What starting a sub-agent failed of, as the issuer is told it, by its class
/// (`SubAgentError::class`), as the tool tells its model: what they can correct as their
/// arguments, a refusal as a refusal, what failed as a failure.
fn command_error(error: SubAgentError) -> CommandError {
    let reason = error.to_string();
    match error.class() {
        ErrorClass::Correctable => invalid(reason),
        ErrorClass::Refused => {
            CommandError::PermissionDenied { command: SUBAGENT_COMMAND_NAME.to_string(), reason }
        }
        ErrorClass::Failed => failed(reason),
    }
}

impl Command for SubagentCommand {
    fn description(&self) -> &CommandDescription {
        &self.description
    }
}

#[async_trait]
impl AgentCommand for SubagentCommand {
    /// The sub-agent started at the message the agent is then given, in the thread the command was
    /// issued in; one whose start cannot be told to the agent is ended again.
    async fn execute(&self, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let (personality, brief) = self.read(args)?;
        let writer = turn.writer::<dyn SubAgentWriter>().await;
        let writer =
            writer.ok_or_else(|| failed("no sub-agent can be started here".to_string()))?;
        let situation = turn.situation();
        let origin = ThreadOrigin { thread: situation.thread_id(), message: MessageId::new() };
        let started = writer.start(personality.as_str(), origin, brief.to_string()).await;
        let thread = started.map_err(command_error)?;
        let content = match self.started(personality, brief, thread) {
            Ok(content) => content,
            Err(error) => {
                if let Err(ending) = writer.end(thread).await {
                    warn!("The sub-agent of thread {} was left started: {}", thread, ending);
                }
                return Err(error);
            }
        };
        // In the name of who asked, as the channel knows them.
        let given = Message {
            id: origin.message,
            sender_id: situation.initiator(),
            sender_name: situation.initiator_name().await.unwrap_or_else(|| "a user".to_string()),
            content: Some(content),
            ..Message::new(situation.classification())
        };
        turn.give(given, None);
        Ok(CommandOutput::private(format!(
            "{} started a sub-agent of '{}' in thread {}.",
            turn.agent_name(),
            personality,
            thread
        )))
    }
}
