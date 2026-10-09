//! The `skills` component (issue 22): what an agent can look up - skills,
//! procedures written down for it to follow - and how a skill reaches it.
//!
//! - **The catalogue**, a prompt part: name and description of every skill
//!   the agent may load where it is (`Views::usable_skills`), in the system
//!   prompt by default (parameter `position`) - it changes rarely, and the
//!   model has to know what it could load. Classified as the join of the
//!   skills it lists.
//! - **`list_skills`** gives the catalogue as a tool, for a model whose
//!   prompt was built before a skill appeared; **`load_skill`** gives a
//!   skill's text as the call's result, its content hidden from the channel,
//!   classified as the skill.
//! - **`/skill @agent <name>`**, a command a member gives the agent: the
//!   skill's text goes into its transcript as a message in the member's
//!   name, read in the command's turn, never into the channel's history;
//!   **`/skills @agent`** lists what it may load.
//!
//! A skill never reaches the model through the system prompt or as a
//! message of the channel: only through the agent's own transcript, so that
//! the prompt cache holds, what it saw is recorded by digest, and nobody in
//! the channel is handed a text they may not read. Every load is recorded
//! as knowledge (`KnowledgeSource::Skill`).

use crate::agent::turn::Turn;
use crate::agent::work::AgentWork;
use crate::command::{AgentCommandProvider, CommandError, CommandResult, ProvidesCommands};
use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{titled, Prompt, PromptPosition, PromptProvider};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::view::{SessionView, SkillView, ViewError};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::question::Answers;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const SKILLS_KIND: &str = "skills";
pub const LIST_SKILLS_TOOL_NAME: &str = "list_skills";
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";
pub const SKILL_COMMAND_NAME: &str = "skill";
pub const SKILLS_COMMAND_NAME: &str = "skills";

const CATALOGUE_TITLE: &str = "Skills you can load";

#[derive(Debug)]
pub struct SkillsComponent {
    pub position: PromptPosition,
}

/// The catalogue of `skills`: a line each, its name and what it is for;
/// empty if there are none.
fn catalogue(skills: &[SkillView]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "A skill is a procedure written down for you: how a task is done here. When a task calls for one of these, \
        load it with `{LOAD_SKILL_TOOL_NAME}` and follow it. A skill a user loaded for you stands in the conversation \
        already, as their message.\n"
    );
    for skill in skills {
        text.push_str(&format!("- {}: {}\n", skill.name(), skill.description()));
    }
    titled(CATALOGUE_TITLE, &text)
}

/// The text of `skill` as the model is given it, the one rendering of both
/// ways in.
fn loaded(skill: &SkillView) -> String {
    titled(&format!("Skill: {}", skill.name()), skill.body().as_str())
}

/// That `skill` was loaded in `channel` of `session`: knowledge of the turn.
fn knowledge(skill: &SkillView, channel: ChannelId, session: SessionId) -> KnowledgeRef {
    KnowledgeRef {
        channel,
        session,
        source: KnowledgeSource::Skill { skill: skill.id(), digest: caelumora_model::digest::skill(skill.body().as_str()) },
        classification: Some(skill.classification().clone()),
    }
}

/// The session `turn` reads from; a turn outside of one - in tests - has
/// no skills.
fn session_of(turn: &Turn<'_>) -> Result<SessionView, ToolError> {
    turn.session().ok_or_else(|| ToolError::ExecutionFailed("the turn is in no session to load skills in".to_string()))
}

#[async_trait]
impl PromptProvider for SkillsComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        let Some(session) = work.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let skills = session.views().usable_skills().await?;
        let classification = Classification::join_all(skills.iter().map(SkillView::classification)).unwrap_or(Classification::PUBLIC);
        Ok(Prompt::classified(catalogue(&skills), classification))
    }
}

#[async_trait]
impl ToolProvider for SkillsComponent {
    fn description(&self) -> String {
        "Skills: procedures written down for you to follow, loaded when a task needs one.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: LIST_SKILLS_TOOL_NAME.to_string(),
                description: "List the skills you can load here, each with what it is for.".to_string(),
                parameters: ToolParameters::object(Vec::<(&str, ToolParameters)>::new(), Vec::<&str>::new()),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            Tool {
                name: LOAD_SKILL_TOOL_NAME.to_string(),
                description: "Load a skill by its name and get its text - a procedure to follow for the task at hand. \
                    The channel sees that you loaded it, not what it says; do not quote it to them."
                    .to_string(),
                parameters: ToolParameters::object([("name", ToolParameters::string("The name of the skill"))], ["name"]),
                requires_approval: false,
                visibility: ToolVisibility::CALL_ONLY,
                classification: None,
                presentation: Default::default(),
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        match name {
            LIST_SKILLS_TOOL_NAME => {
                let skills = session_of(turn)?.views().usable_skills().await?;
                let listed: Vec<Value> = skills.iter().map(|s| serde_json::json!({ "name": s.name(), "description": s.description().as_str() })).collect();
                let classification = Classification::join_all(skills.iter().map(SkillView::classification)).unwrap_or_else(|| turn.label());
                Ok(ToolOutput { value: serde_json::json!({ "skills": listed }), classification: Some(classification) })
            }
            LOAD_SKILL_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Load { name: String }
                let Load { name } = arguments(params)?;
                let skill = session_of(turn)?.views().usable_skill_named(name.trim()).await?;
                turn.record_knowledge(knowledge(&skill, turn.channel_id(), turn.session_id()));
                Ok(ToolOutput { value: Value::String(loaded(&skill)), classification: Some(skill.classification().clone()) })
            }
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

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
fn command_error(command: &str, error: ViewError) -> CommandError {
    match error {
        ViewError::Failed(e) => {
            tracing::warn!("/{} could not read the skills: {:#}", command, e);
            CommandError::Failed { command: command.to_string(), reason: "the skills could not be read".to_string() }
        }
        other => CommandError::InvalidArguments { command: command.to_string(), reason: other.to_string() },
    }
}

#[async_trait]
impl AgentCommandProvider for SkillsComponent {
    async fn execute(&self, name: &str, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let failed = |reason: &str| CommandError::Failed { command: name.to_string(), reason: reason.to_string() };
        let session = turn.session().ok_or_else(|| failed("the agent is in no session"))?;
        match name {
            SKILLS_COMMAND_NAME => {
                let skills = session.views().usable_skills().await.map_err(|e| command_error(name, e))?;
                let text = match skills.is_empty() {
                    true => format!("{} can load no skills here.", turn.agent_name()),
                    false => skills.iter().fold(format!("Skills {} can load here:\n\n", turn.agent_name()), |text, s| text + &format!("- `{}` — {}\n", s.name(), s.description())),
                };
                Ok(CommandOutput::private(text))
            }
            SKILL_COMMAND_NAME => {
                let wanted = args.trim();
                if wanted.is_empty() {
                    return Err(CommandError::InvalidArguments { command: name.to_string(), reason: "name the skill".to_string() });
                }
                let skill = session.views().usable_skill_named(wanted).await.map_err(|e| command_error(name, e))?;
                // In the name of who asked, as the channel knows them.
                let issuer = turn.security_context().initiator_id;
                let issuer_name = match session.channel(turn.channel_id()).await {
                    Ok(channel) => channel.member(issuer).await.ok().and_then(|m| m.name().map(str::to_string)),
                    Err(_) => None,
                };
                let given = Message {
                    id: MessageId::new(),
                    sender_id: issuer,
                    sender_name: issuer_name.unwrap_or_else(|| "a user".to_string()),
                    role: MessageRole::User,
                    content: Some(loaded(&skill)),
                    timestamp: chrono::Utc::now(),
                    ..Message::new(skill.classification().clone())
                };
                // Nothing can fail from here on: what is given is written at once.
                turn.record_knowledge(knowledge(&skill, turn.channel_id(), turn.session_id()));
                turn.give(given);
                Ok(CommandOutput::private(format!("{} has the skill '{}' now.", turn.agent_name(), skill.name())))
            }
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(SKILLS_KIND)
            .provides::<dyn PromptProvider>()
            .provides::<dyn ToolProvider>()
            .provides::<dyn AgentCommandProvider>(),
        |descriptor, _context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let component = Arc::new(SkillsComponent { position });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn ToolProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn AgentCommandProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;

    #[tokio::test]
    async fn the_catalogue_lists_what_may_be_loaded_and_load_skill_gives_its_text_as_knowledge() {
        let world = session_with_workspace().await;
        let component = SkillsComponent { position: PromptPosition::Start };
        let mut state = AgentChannelState::default();
        let prompt = component.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert_eq!(prompt.text, "", "nothing to list, no part");

        let internal = Classification::general(Confidentiality::Internal);
        let made = world.runtime.skill_manager()
            .create(&SecurityContext::system(), "release-notes", Scope::GlobalScope, "How we write a release note", "1. Collect.\n2. Group.", internal.clone()).await.unwrap();
        let mut state = AgentChannelState::default();
        let prompt = component.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.starts_with("[Skills you can load]") && prompt.text.contains("- release-notes: How we write a release note"), "{}", prompt.text);
        assert!(!prompt.text.contains("Collect"), "the catalogue, not the text");
        assert_eq!(prompt.classification, Some(internal.clone()));

        let mut turn = world.turn(&mut state);
        let loaded = ToolProvider::execute(&component, LOAD_SKILL_TOOL_NAME, &serde_json::json!({ "name": "release-notes" }), None, &mut turn).await.unwrap();
        assert_eq!(loaded.value, Value::String("[Skill: release-notes]\n1. Collect.\n2. Group.\n".to_string()), "the text, under its title");
        assert_eq!(loaded.classification, Some(internal));
        drop(turn);
        let read = state.turn.as_ref().unwrap().work.context.knowledge.clone();
        assert!(matches!(read.as_slice(), [KnowledgeRef { source: KnowledgeSource::Skill { skill, digest }, .. }] if *skill == made.id && *digest == caelumora_model::digest::skill("1. Collect.\n2. Group.")), "{read:?}");

        let mut turn = world.turn(&mut state);
        let absent = ToolProvider::execute(&component, LOAD_SKILL_TOOL_NAME, &serde_json::json!({ "name": "nothing" }), None, &mut turn).await;
        assert!(matches!(absent, Err(ToolError::InvalidParameter(_))), "the model's to correct");
    }

    #[test]
    fn load_skill_shows_the_channel_that_it_was_called_not_what_it_gave() {
        let tools = SkillsComponent { position: PromptPosition::Start }.tools();
        let load = tools.iter().find(|t| t.name == LOAD_SKILL_TOOL_NAME).unwrap();
        assert_eq!(load.visibility, ToolVisibility::CALL_ONLY);
        assert!(tools.iter().all(|t| !t.requires_approval), "reading needs no approval");
    }
}
