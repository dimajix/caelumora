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
//!   skill's text as the call's result, hidden from the channel, which sees
//!   only which skill was loaded (issue 281), classified as the skill.
//! - **`create_skill`** and **`update_skill`** let the agent write down
//!   what it has learnt to do (issue 22, decision 6): a skill of its owner's,
//!   in the session's scope or the owner's - never the global one -,
//!   classified as what the turn has seen; changing one raises its
//!   classification to that, so that nothing the turn saw leaks into it.
//!   Both are approved unless the parameter `requires_approval` says
//!   otherwise; only the owner's own skills can be changed (`UpdateSkill`).
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
use crate::skill::SkillError;
use crate::tool::{arguments, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::view::{SessionView, SkillView, ViewError};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::question::Answers;
use caelumora_model::scope::Scope;
use caelumora_model::store::StoreError;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const SKILLS_KIND: &str = "skills";
pub const LIST_SKILLS_TOOL_NAME: &str = "list_skills";
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";
pub const CREATE_SKILL_TOOL_NAME: &str = "create_skill";
pub const UPDATE_SKILL_TOOL_NAME: &str = "update_skill";
pub const SKILL_COMMAND_NAME: &str = "skill";
pub const SKILLS_COMMAND_NAME: &str = "skills";

const CATALOGUE_TITLE: &str = "Skills you can load";

#[derive(Debug)]
pub struct SkillsComponent {
    pub(crate) position: PromptPosition,
    /// Whether writing a skill is approved first: always, unless the
    /// parameter `requires_approval` says `false`.
    pub(crate) requires_approval: ApprovalMode,
}

/// What the component is configured with besides its position.
#[derive(Debug, Default, Deserialize)]
struct SkillsConfig {
    #[serde(default)]
    requires_approval: ApprovalMode,
}

/// The classification of what `skills` hold together, within `bound` -
/// the turn's: their join where it surely flows there, else the bound.
fn joined(skills: &[SkillView], bound: &Classification) -> Classification {
    skills.iter().fold(Classification::PUBLIC, |joined, skill| joined.join_within(skill.classification(), bound))
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

/// What `create_skill` is called with.
#[derive(Deserialize)]
struct Create {
    name: String,
    description: String,
    body: String,
    scope: String,
}

/// What `update_skill` is called with.
#[derive(Deserialize)]
struct Update {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

/// Where an agent may write a skill (issue 22, decision 6): never the
/// global scope.
enum SkillScope {
    Session,
    User,
}

impl SkillScope {
    fn of(scope: &str) -> ToolResult<Self> {
        match scope.trim() {
            "session" => Ok(SkillScope::Session),
            "user" => Ok(SkillScope::User),
            other => Err(ToolError::InvalidParameter(format!("scope must be \"session\" or \"user\", not \"{other}\""))),
        }
    }

    /// How an approver is told where it goes.
    fn told(&self) -> &'static str {
        match self {
            SkillScope::Session => "for this session alone",
            SkillScope::User => "for every session of the agent's owner",
        }
    }
}

/// What the model is told when writing a skill failed: what it can
/// correct, such as a name taken, text that does not fit or a skill not its
/// owner's, as such; the rest as a failure.
fn tool_error(error: SkillError) -> ToolError {
    match error {
        SkillError::Invalid(e) => ToolError::InvalidParameter(e.to_string()),
        SkillError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter("a skill of that name exists there already; update it instead".to_string()),
        SkillError::Store(StoreError::PermissionDenied) => ToolError::PermissionDenied("you may write only skills of your owner's".to_string()),
        SkillError::NotFound(_) => ToolError::InvalidParameter("there is no such skill you may change".to_string()),
        SkillError::OwnerNotAUser => ToolError::InvalidParameter("a skill belongs to a user, and your owner is none".to_string()),
        SkillError::ClassificationRefused | SkillError::NotCleared => ToolError::PermissionDenied("the skill may not be classified as what you have seen".to_string()),
        SkillError::Store(e) => ToolError::ExecutionFailed(format!("the skill could not be stored: {e}")),
    }
}

#[async_trait]
impl PromptProvider for SkillsComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        let Some(session) = work.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let skills = session.views().usable_skills().await?;
        let classification = joined(&skills, &work.classification());
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
                    The channel sees which skill you loaded, not what it says; do not quote it to them."
                    .to_string(),
                parameters: ToolParameters::object([("name", ToolParameters::string("The name of the skill"))], ["name"]),
                requires_approval: false,
                visibility: ToolVisibility::WITHOUT_RESULT,
                classification: None,
                presentation: ToolPresentation::Skill,
            },
            Tool {
                name: CREATE_SKILL_TOOL_NAME.to_string(),
                description: "Write down a skill: how a task is done here, as you have learnt it - steps to follow, \
                    for yourself and other agents of your owner's next time. A skill is for what to do, not for what you \
                    know (that is memory). Keep it self-contained; check the skills you can load first, and update one \
                    rather than writing a second. It belongs to your owner, and lies in this session's scope or your \
                    owner's. A user has to approve it."
                    .to_string(),
                parameters: ToolParameters::object(
                    [
                        ("name", ToolParameters::string("A short name, e.g. release-notes")),
                        ("description", ToolParameters::string("What it is for, in one line: what the catalogue shows")),
                        ("body", ToolParameters::string("The steps, in Markdown")),
                        ("scope", ToolParameters::string("\"session\" for this session alone, \"user\" for every session of your owner")),
                    ],
                    ["name", "description", "body", "scope"],
                ),
                requires_approval: self.requires_approval.required(false),
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: ToolPresentation::Skill,
            },
            Tool {
                name: UPDATE_SKILL_TOOL_NAME.to_string(),
                description: "Change a skill you can load here that belongs to your owner: what it is for, its steps, \
                    or both. Give the whole new text of what you change. A user has to approve it."
                    .to_string(),
                parameters: ToolParameters::object(
                    [
                        ("name", ToolParameters::string("The name of the skill")),
                        ("description", ToolParameters::string("What it is for, in one line, if that changes")),
                        ("body", ToolParameters::string("The steps, in Markdown, if they change")),
                    ],
                    ["name"],
                ),
                requires_approval: self.requires_approval.required(false),
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: ToolPresentation::Skill,
            },
        ]
    }

    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        match name {
            CREATE_SKILL_TOOL_NAME => {
                let Create { name, description, body, scope } = arguments(params)?;
                let scope = SkillScope::of(&scope)?;
                Ok(format!("Write the skill \"{name}\" ({}): {description}\n\n{body}", scope.told()))
            }
            UPDATE_SKILL_TOOL_NAME => {
                let Update { name, description, body } = arguments(params)?;
                if description.is_none() && body.is_none() {
                    return Err(ToolError::InvalidParameter("give a new description, new steps, or both".to_string()));
                }
                let mut text = format!("Change the skill \"{name}\"");
                if let Some(description) = description {
                    text.push_str(&format!(" - what it is for, from now on: {description}"));
                }
                if let Some(body) = body {
                    text.push_str(&format!("\n\nIts steps, from now on:\n\n{body}"));
                }
                Ok(text)
            }
            _ => Ok(self.tools().into_iter().find(|t| t.name == name).map(|t| t.description).unwrap_or_default()),
        }
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        match name {
            LIST_SKILLS_TOOL_NAME => {
                let skills = session_of(turn)?.views().usable_skills().await?;
                let listed: Vec<Value> = skills.iter().map(|s| serde_json::json!({ "name": s.name(), "description": s.description().as_str() })).collect();
                Ok(ToolOutput { value: serde_json::json!({ "skills": listed }), classification: Some(joined(&skills, &turn.classification())) })
            }
            LOAD_SKILL_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Load { name: String }
                let Load { name } = arguments(params)?;
                let skill = session_of(turn)?.views().usable_skill_named(name.trim()).await?;
                turn.record_knowledge(knowledge(&skill, turn.channel_id(), turn.session_id()));
                Ok(ToolOutput { value: Value::String(loaded(&skill)), classification: Some(skill.classification().clone()) })
            }
            CREATE_SKILL_TOOL_NAME => {
                let Create { name, description, body, scope } = arguments(params)?;
                let session = session_of(turn)?;
                let context = turn.security_context().clone();
                let scope = match SkillScope::of(&scope)? {
                    SkillScope::Session => Scope::SessionScope { session_id: turn.session_id() },
                    SkillScope::User => match context.principal_id.as_user_id() {
                        Some(user_id) => Scope::UserScope { user_id },
                        None => return Err(ToolError::InvalidParameter("your owner is no user: write a skill of this session".to_string())),
                    },
                };
                let made = session.views().skill_manager()
                    .create(&context, name.trim(), scope, &description, &body, turn.label()).await
                    .map_err(tool_error)?;
                Ok(ToolOutput::classified(serde_json::json!({ "status": "created", "name": made.name.name.as_str(), "skill_id": made.id }), turn.label()))
            }
            UPDATE_SKILL_TOOL_NAME => {
                let Update { name, description, body } = arguments(params)?;
                if description.is_none() && body.is_none() {
                    return Err(ToolError::InvalidParameter("give a new description, new steps, or both".to_string()));
                }
                let session = session_of(turn)?;
                let skill = session.views().usable_skill_named(name.trim()).await?;
                let context = turn.security_context().clone();
                let manager = session.views().skill_manager();
                // What the turn saw goes into the skill: raised to it first,
                // so that nothing of it is ever held lower.
                let raised = skill.classification().join_within(&turn.label(), &turn.classification());
                if raised != *skill.classification() {
                    manager.reclassify(&context, skill.id(), raised.clone()).await.map_err(tool_error)?;
                }
                if let Some(description) = description {
                    manager.describe(&context, skill.id(), &description).await.map_err(tool_error)?;
                }
                if let Some(body) = body {
                    manager.rewrite(&context, skill.id(), &body).await.map_err(tool_error)?;
                }
                Ok(ToolOutput::classified(serde_json::json!({ "status": "updated", "name": skill.name(), "skill_id": skill.id(), "classification": raised.to_string() }), turn.label()))
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
            let SkillsConfig { requires_approval } = crate::component::config_of_params(&descriptor.params)?;
            let component = Arc::new(SkillsComponent { position, requires_approval });
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
        let component = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Always };
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

    async fn call(component: &SkillsComponent, turn: &mut Turn<'_>, name: &str, params: Value) -> ToolResult<ToolOutput> {
        ToolProvider::execute(component, name, &params, None, turn).await
    }

    #[tokio::test]
    async fn an_agent_writes_a_skill_of_its_owners_classified_as_what_it_saw_and_raises_one_it_changes() {
        let world = session_with_workspace().await;
        let component = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Always };
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let created = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.", "scope": "session" })).await.unwrap();
        assert_eq!(created.value["status"], "created");
        let created_id = created.value["skill_id"].clone();
        let global = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "other", "description": "d", "body": "b", "scope": "global" })).await;
        assert!(matches!(global, Err(ToolError::InvalidParameter(_))), "never global");
        let twice = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "Deploy", "description": "d", "body": "b", "scope": "session" })).await;
        assert!(matches!(twice, Err(ToolError::InvalidParameter(ref m)) if m.contains("update it instead")), "{twice:?}");
        let mine = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "notes", "description": "How we take notes", "body": "Short.", "scope": "user" })).await;
        assert!(mine.is_ok(), "{mine:?}");
        drop(turn);

        let skills = world.runtime.entity_store().list_skills(&SecurityContext::system()).await.unwrap();
        let deploy = skills.iter().find(|s| s.name.name.as_str() == "deploy").expect("made");
        let owner = world.context.principal_id;
        assert_eq!((deploy.security_attributes.owner_id, &deploy.name.scope), (owner, &Scope::SessionScope { session_id: world.session.id() }), "its owner's, in the session");
        assert_eq!(deploy.classification, Classification::PUBLIC, "what the turn had seen: nothing above public");
        let notes = skills.iter().find(|s| s.name.name.as_str() == "notes").expect("made");
        assert_eq!(notes.name.scope, Scope::UserScope { user_id: owner.as_user_id().unwrap() });

        // A turn that saw something internal writes it into the skill: raised first.
        let internal = Classification::general(Confidentiality::Internal);
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        turn.see(&internal);
        let updated = call(&component, &mut turn, UPDATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy", "body": "1. Build.\n2. Ship." })).await.unwrap();
        assert_eq!(updated.value["status"], "updated");
        assert_eq!(updated.value["skill_id"], created_id, "the skill it changed, for a frontend to link to");
        let nothing = call(&component, &mut turn, UPDATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy" })).await;
        assert!(matches!(nothing, Err(ToolError::InvalidParameter(_))), "nothing to change");
        drop(turn);
        let deploy = world.runtime.entity_store().get_skill(&SecurityContext::system(), deploy.id).await.unwrap();
        assert_eq!((deploy.body.as_str(), &deploy.classification), ("1. Build.\n2. Ship.", &internal), "the new steps, classified as what was seen");
        assert_eq!(created_id, serde_json::json!(deploy.id), "the skill it made");
    }

    #[test]
    fn an_approver_is_shown_the_skill_whole() {
        let component = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Always };
        let shown = component.describe(CREATE_SKILL_TOOL_NAME, &serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.\n2. Ship.", "scope": "user" })).unwrap();
        assert_eq!(shown, "Write the skill \"deploy\" (for every session of the agent's owner): How we deploy\n\n1. Build.\n2. Ship.");
        let tools = component.tools();
        assert!(tools.iter().filter(|t| t.name == CREATE_SKILL_TOOL_NAME || t.name == UPDATE_SKILL_TOOL_NAME).all(|t| t.requires_approval), "approved by default");
        let unapproved = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Never }.tools();
        assert!(unapproved.iter().all(|t| !t.requires_approval), "unless configured otherwise");
    }

    #[test]
    fn load_skill_shows_the_channel_which_skill_it_loaded_not_what_it_gave() {
        let tools = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Always }.tools();
        let load = tools.iter().find(|t| t.name == LOAD_SKILL_TOOL_NAME).unwrap();
        assert_eq!(load.visibility, ToolVisibility::WITHOUT_RESULT);
        let skills: Vec<_> = tools.iter().filter(|t| t.presentation == ToolPresentation::Skill).map(|t| t.name.as_str()).collect();
        assert_eq!(skills, [LOAD_SKILL_TOOL_NAME, CREATE_SKILL_TOOL_NAME, UPDATE_SKILL_TOOL_NAME], "shown as a skill; the catalogue as a call");
        assert!(tools.iter().filter(|t| t.name == LOAD_SKILL_TOOL_NAME || t.name == LIST_SKILLS_TOOL_NAME).all(|t| !t.requires_approval), "reading needs no approval");
    }
}
