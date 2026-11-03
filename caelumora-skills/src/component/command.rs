//! The facet `AgentCommandProvider` of `skills` (issue 22): **`/skill @agent <name>`**, a command a
//! participant gives the agent - the skill's text goes into its transcript as a message in the
//! participant's name, read in the command's turn, never into the channel's history -; and
//! **`/skills @agent`**, which lists what it may load.

use super::{reader_in, SkillsComponent};
use crate::extension::SkillError;
use async_trait::async_trait;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::turn::Turn;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_domain::command::{Command, CommandOutput};
use caelumora_domain::message::{Message, MessageId, MessageRole};
use chrono::Utc;
use tracing::warn;

/// The command that gives the agent a skill.
pub const SKILL_COMMAND_NAME: &str = "skill";
/// The command that lists the skills the agent may load.
const SKILLS_COMMAND_NAME: &str = "skills";

impl ProvidesCommands for SkillsComponent {
    fn commands(&self) -> Vec<Command> {
        vec![
            Command {
                name: SKILL_COMMAND_NAME.to_string(),
                description: "Gives the agent a skill to follow: its text goes into the agent's conversation, in your name.".to_string(),
                usage: Some("<name>".to_string()),
                owners_only: false,
            },
            Command {
                name: SKILLS_COMMAND_NAME.to_string(),
                description: "Lists the skills the agent can load here.".to_string(),
                usage: None,
                owners_only: false,
            },
        ]
    }
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

#[async_trait]
impl AgentCommandProvider for SkillsComponent {
    async fn execute(
        &self,
        name: &str,
        args: &str,
        turn: &mut Turn<'_>,
    ) -> CommandResult<CommandOutput> {
        let failed = |reason: &str| CommandError::Failed {
            command: name.to_string(),
            reason: reason.to_string(),
        };
        let skills = reader_in(&turn.situation())
            .await
            .ok_or_else(|| failed("the agent is in no session"))?;
        match name {
            SKILLS_COMMAND_NAME => {
                let skills = skills.usable_skills().await.map_err(|e| command_error(name, e))?;
                let text = match skills.is_empty() {
                    true => format!("{} can load no skills here.", turn.agent_name()),
                    false => skills.iter().fold(
                        format!("Skills {} can load here:\n\n", turn.agent_name()),
                        |text, s| text + &format!("- `{}` — {}\n", s.name.name, s.description),
                    ),
                };
                Ok(CommandOutput::private(text))
            }
            SKILL_COMMAND_NAME => {
                let wanted = args.trim();
                if wanted.is_empty() {
                    return Err(CommandError::InvalidArguments {
                        command: name.to_string(),
                        reason: "name the skill".to_string(),
                    });
                }
                let skill =
                    skills.usable_skill_named(wanted).await.map_err(|e| command_error(name, e))?;
                // In the name of who asked, as the channel knows them.
                let issuer = turn.situation().initiator();
                let issuer_name = match turn.situation().channel().await {
                    Ok(Some(channel)) => channel
                        .participant(issuer)
                        .await
                        .ok()
                        .and_then(|m| m.name().map(str::to_string)),
                    Ok(None) | Err(_) => None,
                };
                let given = Message {
                    id: MessageId::new(),
                    sender_id: issuer,
                    sender_name: issuer_name.unwrap_or_else(|| "a user".to_string()),
                    role: MessageRole::User,
                    content: Some(self.given(&skill).map_err(|e| failed(&e.to_string()))?),
                    timestamp: Utc::now(),
                    ..Message::new(skill.classification.clone())
                };
                // Nothing can fail from here on: what is given is written at once.
                let knowledge =
                    skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
                turn.give(given, Some(knowledge));
                Ok(CommandOutput::private(format!(
                    "{} has the skill '{}' now.",
                    turn.agent_name(),
                    skill.name.name
                )))
            }
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}
