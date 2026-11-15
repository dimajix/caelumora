//! The commands of `skills` (issue 22): **`/skill @agent <name>`**, a command a participant gives
//! the agent - the skill's text goes into its transcript as given to read, in the participant's
//! name, read in the command's turn, and into the channel's history as what the agent was given,
//! gone to nobody (issue 417); not again where it stands so already (issue 541) -; and
//! **`/skills @agent`**, which lists what it may load.

use super::{reader_in, SkillsComponent};
use crate::extension::{SkillError, SkillReader};
use async_trait::async_trait;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::turn::Turn;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;
use chrono::Utc;
use std::sync::Arc;
use tracing::warn;

/// The command that gives the agent a skill.
pub const SKILL_COMMAND_NAME: &str = "skill";
/// The command that lists the skills the agent may load.
const SKILLS_COMMAND_NAME: &str = "skills";

/// The commands of the component `skills`, the first giving a skill as `skills` words it.
pub(super) fn commands(skills: &Arc<SkillsComponent>) -> Vec<Arc<dyn AgentCommand>> {
    vec![Arc::new(SkillCommand::new(Arc::clone(skills))), Arc::new(SkillsCommand::new())]
}

/// What a command answers when a skill cannot be reached: one it may not
/// load is the issuer's to correct, as a name that does not exist; a
/// failure to read is the command's.
fn command_error(command: &str, error: SkillError) -> CommandError {
    match error {
        SkillError::Store(_) | SkillError::NotJoined => {
            warn!("/{} could not read the skills: {:#}", command, error);
            CommandError::Failed {
                command: command.to_string(),
                reason: "the skills could not be read".to_string(),
            }
        }
        other => CommandError::InvalidArguments {
            command: command.to_string(),
            reason: other.to_string(),
        },
    }
}

/// The skills the agent may load in the work of `turn`, for the command `command`.
///
/// # Errors
/// `CommandError::Failed` if the agent reaches no session.
async fn usable_in(command: &str, turn: &Turn<'_>) -> CommandResult<Arc<dyn SkillReader>> {
    reader_in(&turn.situation()).await.ok_or_else(|| CommandError::Failed {
        command: command.to_string(),
        reason: "the agent is in no session".to_string(),
    })
}

/// `/skill @agent <name>`: gives the agent the skill named.
#[derive(Debug)]
struct SkillCommand {
    description: CommandDescription,
    /// What words the skill as it is given.
    skills: Arc<SkillsComponent>,
}

impl SkillCommand {
    /// The command, giving skills as `skills` words them.
    fn new(skills: Arc<SkillsComponent>) -> Self {
        let description = CommandDescription {
            name: CommandName::fixed(SKILL_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Gives the agent a skill to follow: its text goes into the agent's conversation, in your name.".to_string(),
            usage: Some("<name>".to_string()),
            owners_only: false,
        };
        Self { description, skills }
    }
}

impl Command for SkillCommand {
    fn description(&self) -> &CommandDescription {
        &self.description
    }
}

#[async_trait]
impl AgentCommand for SkillCommand {
    async fn execute(&self, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let name = SKILL_COMMAND_NAME;
        let skills = usable_in(name, turn).await?;
        let wanted = args.trim();
        if wanted.is_empty() {
            return Err(CommandError::InvalidArguments {
                command: name.to_string(),
                reason: "name the skill".to_string(),
            });
        }
        let skill = skills.usable_skill_named(wanted).await.map_err(|e| command_error(name, e))?;
        // In the name of who asked, as the channel knows them.
        let issuer = turn.situation().initiator();
        let issuer_name = match turn.situation().channel().await {
            Ok(Some(channel)) => {
                channel.participant(issuer).await.ok().and_then(|m| m.name().map(Name::to_string))
            }
            Ok(None) | Err(_) => None,
        };
        let content = self.skills.given(&skill).map_err(|e| CommandError::Failed {
            command: name.to_string(),
            reason: e.to_string(),
        })?;
        let given = Message {
            id: MessageId::new(),
            sender_id: issuer,
            sender_name: issuer_name.unwrap_or_else(|| "a user".to_string()),
            content: Some(content),
            timestamp: Utc::now(),
            ..Message::new(skill.classification.clone())
        };
        // Nothing can fail from here on: what is given is written at once.
        let knowledge =
            skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
        let when = if turn.give(given, Some(knowledge)) { "now" } else { "already" };
        Ok(CommandOutput::private(format!(
            "{} has the skill '{}' {}.",
            turn.agent_name(),
            skill.name.name,
            when
        )))
    }
}

/// `/skills @agent`: lists the skills the agent may load here.
#[derive(Debug)]
struct SkillsCommand(CommandDescription);

impl SkillsCommand {
    /// The command, as `/help` tells it.
    fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(SKILLS_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Lists the skills the agent can load here.".to_string(),
            usage: None,
            owners_only: false,
        })
    }
}

impl Command for SkillsCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl AgentCommand for SkillsCommand {
    async fn execute(&self, _args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let name = SKILLS_COMMAND_NAME;
        let skills = usable_in(name, turn).await?;
        let skills = skills.usable_skills().await.map_err(|e| command_error(name, e))?;
        let text = match skills.is_empty() {
            true => format!("{} can load no skills here.", turn.agent_name()),
            false => skills
                .iter()
                .fold(format!("Skills {} can load here:\n\n", turn.agent_name()), |text, s| {
                    text + &format!("- `{}` — {}\n", s.name.name, s.description)
                }),
        };
        Ok(CommandOutput::private(text))
    }
}
