//! The `skills` component (issue 22): what an agent can look up - skills,
//! procedures written down for it to follow - and how a skill reaches it.
//!
//! - **The catalogue**, a prompt part: name and description of every skill
//!   the agent may load where it is (`Views::usable_skills`), in the system
//!   prompt by default (parameter `position`) - it changes rarely, and the
//!   model has to know what it could load. Classified as the join of the
//!   skills it lists.
//! - **`list_skills`** gives the catalogue as a tool, for a model whose
//!   prompt was built before a skill appeared - each skill with its id, for
//!   the channel to link to (issue 301); **`load_skill`** gives a
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
//! as knowledge (`KnowledgeSource::Entity`), and a skill loaded in what a
//! compaction summarises is given again, as it is now - where it may still
//! be loaded (`KnowledgeRestorer`, issue 323).

use crate::manager::SkillError;
use crate::skill::{Skill, SkillId};
use crate::view::{SkillView, SkillViews};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::question::Answers;
use caelumora_model::scope::Scope;
use caelumora_model::store::{EntityKind, StoreError, Stored};
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_agents::turn::Turn;
use caelumora_agents::situation::Situation;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_runtime::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_agents::compaction::{CompactionError, GivenAgain, KnowledgeRestorer};
use caelumora_runtime::component::{config_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use caelumora_agents::prompt::{ContextError, titled, Prompt, PromptPosition, PromptProvider};
use caelumora_agents::tool::{arguments, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use caelumora_runtime::view::SessionView;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

/// A skill loaded, written or changed (issue 281): its `name`, and
/// where the arguments hold them its `description` and its `body`, as
/// Markdown; its result's `skill_id`, where it has one, to link to.
pub const SKILL_PRESENTATION: ToolPresentation = ToolPresentation::new("skill");

/// The skills an agent may load, listed (issue 301): the `skills` of its
/// result - each `name`, `description` and `skill_id`, to link to.
pub const SKILL_LIST_PRESENTATION: ToolPresentation = ToolPresentation::new("skill_list");

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
    pub position: PromptPosition,
    /// Whether writing a skill is approved first: always, unless the
    /// parameter `requires_approval` says `false`.
    pub requires_approval: ApprovalMode,
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

/// The session `turn` reads from (`Situation::session`); a turn that
/// reaches none has no skills.
async fn session_of(turn: &Turn<'_>) -> Result<SessionView, ToolError> {
    turn.situation().session().await?.ok_or_else(|| ToolError::ExecutionFailed("the turn is in no session to load skills in".to_string()))
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

/// Why the catalogue of skills cannot be given (issue 336): what the store
/// reported, judged as store errors are; the rest in the words it has,
/// which are written to be read.
impl From<SkillError> for ContextError {
    fn from(error: SkillError) -> Self {
        match error {
            SkillError::Store(e) => ContextError::Store(e),
            error => ContextError::Failed(error.to_string()),
        }
    }
}

/// What the model is told when reaching or writing a skill failed: what it
/// can correct, such as a name taken, text that does not fit or a skill not
/// its owner's, as such; the rest as a failure.
impl From<SkillError> for ToolError {
    fn from(error: SkillError) -> Self {
        match error {
            SkillError::Invalid(e) => ToolError::InvalidParameter(e.to_string()),
            SkillError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter("a skill of that name exists there already; update it instead".to_string()),
            SkillError::Store(StoreError::PermissionDenied(_)) => ToolError::PermissionDenied("you may write only skills of your owner's".to_string()),
            SkillError::NotFound(_) => ToolError::InvalidParameter("there is no such skill you may change".to_string()),
            SkillError::NoSkillNamed(_) => ToolError::InvalidParameter(error.to_string()),
            SkillError::OwnerNotAUser => ToolError::InvalidParameter("a skill belongs to a user, and your owner is none".to_string()),
            SkillError::ClassificationRefused | SkillError::NotCleared => ToolError::PermissionDenied("the skill may not be classified as what you have seen".to_string()),
            SkillError::NotJoined => ToolError::ExecutionFailed(error.to_string()),
            SkillError::Store(e) => ToolError::ExecutionFailed(format!("the skill could not be stored: {e}")),
        }
    }
}

#[async_trait]
impl PromptProvider for SkillsComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let Some(session) = situation.session().await? else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let skills = session.views().usable_skills().await?;
        let classification = joined(&skills, &situation.classification());
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
                presentation: SKILL_LIST_PRESENTATION,
            },
            Tool {
                name: LOAD_SKILL_TOOL_NAME.to_string(),
                description: "Load a skill by its name - a procedure to follow for the task at hand. Its text follows the \
                    result. The channel sees which skill you loaded, not what it says; do not quote it to them."
                    .to_string(),
                parameters: ToolParameters::object([("name", ToolParameters::string("The name of the skill"))], ["name"]),
                requires_approval: false,
                // Its result names the skill; its text is given to the agent,
                // never published (issue 389).
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: SKILL_PRESENTATION,
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
                presentation: SKILL_PRESENTATION,
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
                presentation: SKILL_PRESENTATION,
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
                let skills = session_of(turn).await?.views().usable_skills().await?;
                let listed: Vec<Value> = skills.iter().map(|s| serde_json::json!({ "name": s.name(), "description": s.description().as_str(), "skill_id": s.id() })).collect();
                Ok(ToolOutput { value: serde_json::json!({ "skills": listed }), classification: Some(joined(&skills, &turn.situation().classification())) })
            }
            LOAD_SKILL_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Load { name: String }
                let Load { name } = arguments(params)?;
                let skill = session_of(turn).await?.views().usable_skill_named(name.trim()).await?;
                // Its text is given after the step's results, as a skill a
                // user loads is (issue 389): the transcript holds every skill
                // as what it is, for a compaction to give it again. The
                // result only names it, classified as the skill, as the
                // catalogue names it.
                let given = Message {
                    id: MessageId::new(),
                    sender_id: ActorId::Agent(turn.situation().agent_id()),
                    sender_name: turn.agent_name().to_string(),
                    role: MessageRole::User,
                    content: Some(skill.as_given()),
                    timestamp: Utc::now(),
                    ..Message::new(skill.classification().clone())
                };
                let knowledge = skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
                turn.give(given, Some(knowledge));
                Ok(ToolOutput { value: serde_json::json!({ "status": "loaded", "name": skill.name(), "skill_id": skill.id() }), classification: Some(skill.classification().clone()) })
            }
            CREATE_SKILL_TOOL_NAME => {
                let Create { name, description, body, scope } = arguments(params)?;
                let session = session_of(turn).await?;
                let context = turn.situation().security_context().clone();
                let scope = match SkillScope::of(&scope)? {
                    SkillScope::Session => Scope::SessionScope { session_id: turn.situation().session_id() },
                    SkillScope::User => match context.principal_id.as_user_id() {
                        Some(user_id) => Scope::UserScope { user_id },
                        None => return Err(ToolError::InvalidParameter("your owner is no user: write a skill of this session".to_string())),
                    },
                };
                let made = session.views().skill_manager()?
                    .create(&context, name.trim(), scope, &description, &body, turn.situation().label()).await?;
                Ok(ToolOutput::classified(serde_json::json!({ "status": "created", "name": made.name.name.as_str(), "skill_id": made.id }), turn.situation().label()))
            }
            UPDATE_SKILL_TOOL_NAME => {
                let Update { name, description, body } = arguments(params)?;
                if description.is_none() && body.is_none() {
                    return Err(ToolError::InvalidParameter("give a new description, new steps, or both".to_string()));
                }
                let session = session_of(turn).await?;
                let skill = session.views().usable_skill_named(name.trim()).await?;
                let context = turn.situation().security_context().clone();
                let views = session.views();
                let manager = views.skill_manager()?;
                // What the turn saw goes into the skill: raised to it first,
                // so that nothing of it is ever held lower.
                let raised = skill.classification().join_within(&turn.situation().label(), &turn.situation().classification());
                if raised != *skill.classification() {
                    manager.reclassify(&context, skill.id(), raised.clone()).await?;
                }
                if let Some(description) = description {
                    manager.describe(&context, skill.id(), &description).await?;
                }
                if let Some(body) = body {
                    manager.rewrite(&context, skill.id(), &body).await?;
                }
                Ok(ToolOutput::classified(serde_json::json!({ "status": "updated", "name": skill.name(), "skill_id": skill.id(), "classification": raised.to_string() }), turn.situation().label()))
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
fn command_error(command: &str, error: SkillError) -> CommandError {
    match error {
        SkillError::Store(_) | SkillError::NotJoined => {
            warn!("/{} could not read the skills: {:#}", command, error);
            CommandError::Failed { command: command.to_string(), reason: "the skills could not be read".to_string() }
        }
        other => CommandError::InvalidArguments { command: command.to_string(), reason: other.to_string() },
    }
}

#[async_trait]
impl AgentCommandProvider for SkillsComponent {
    async fn execute(&self, name: &str, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let failed = |reason: &str| CommandError::Failed { command: name.to_string(), reason: reason.to_string() };
        let session = turn.situation().session().await
            .map_err(|e| {
                warn!("/{} could not read the agent's session: {:#}", name, e);
                failed("the agent's session could not be read")
            })?
            .ok_or_else(|| failed("the agent is in no session"))?;
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
                let issuer = turn.situation().security_context().initiator_id;
                let issuer_name = match turn.situation().channel().await {
                    Ok(Some(channel)) => channel.member(issuer).await.ok().and_then(|m| m.name().map(str::to_string)),
                    Ok(None) | Err(_) => None,
                };
                let given = Message {
                    id: MessageId::new(),
                    sender_id: issuer,
                    sender_name: issuer_name.unwrap_or_else(|| "a user".to_string()),
                    role: MessageRole::User,
                    content: Some(skill.as_given()),
                    timestamp: Utc::now(),
                    ..Message::new(skill.classification().clone())
                };
                // Nothing can fail from here on: what is given is written at once.
                let knowledge = skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
                turn.give(given, Some(knowledge));
                Ok(CommandOutput::private(format!("{} has the skill '{}' now.", turn.agent_name(), skill.name())))
            }
            _ => Err(CommandError::Unknown(name.to_string())),
        }
    }
}

#[async_trait]
impl KnowledgeRestorer for SkillsComponent {
    fn kind(&self) -> EntityKind {
        Skill::KIND
    }

    /// The skills of `given` the agent may still load where it is.
    async fn give_again(&self, turn: &mut Turn<'_>, given: &[Uuid]) -> Result<Vec<GivenAgain>, CompactionError> {
        let agent = turn.situation().agent_id();
        let unreadable = |e: &dyn Display| {
            warn!("Agent {}: the skills it had loaded could not be read again: {:#}", agent, e);
            CompactionError::KnowledgeUnreadable
        };
        let session = turn.situation().session().await.map_err(|e| unreadable(&e))?.ok_or(CompactionError::NoSession)?;
        let usable = session.views().usable_skills().await.map_err(|e| unreadable(&e))?;
        Ok(given.iter()
            .filter_map(|id| usable.iter().find(|skill| skill.id() == SkillId::from(*id)))
            .map(|skill| GivenAgain {
                name: skill.name().to_string(),
                text: skill.as_given(),
                knowledge: skill.knowledge(turn.situation().channel_id(), turn.situation().session_id()),
            })
            .collect())
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(SKILLS_KIND)
            .provides::<dyn PromptProvider>()
            .provides::<dyn ToolProvider>()
            .provides::<dyn AgentCommandProvider>()
            .provides::<dyn KnowledgeRestorer>(),
        |descriptor, _context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let SkillsConfig { requires_approval } = config_of_params(&descriptor.params)?;
            let component = Arc::new(SkillsComponent { position, requires_approval });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn ToolProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn AgentCommandProvider>(Arc::<SkillsComponent>::clone(&component));
            facets.insert::<dyn KnowledgeRestorer>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_agents::error::user_message;
    use anyhow::Error;

    #[test]
    fn what_the_catalogue_meets_reaches_the_owner_as_the_core_judges_it() {
        let shown = |error: SkillError| user_message(&Error::new(ContextError::from(error)));
        assert_eq!(shown(SkillError::NotJoined).as_deref(), Some("skills are not offered here"));
        assert_eq!(shown(SkillError::Store(StoreError::Backend("/var/lib/skills: permission denied".to_string()))), None);
    }
}
