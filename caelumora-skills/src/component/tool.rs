//! The facet `ToolProvider` of `skills` (issues 22, 281, 301): the tools that list and load the
//! skills the agent may load, and write and change those of its owner's.
//!
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

use super::{joined, reader_in, SkillsComponent, SKILLS_KIND};
use crate::extension::{SkillError, SkillReader, SkillWriter};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::tool::{
    arguments, described, ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::actor::ActorId;
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreError;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// A skill loaded, written or changed (issue 281): its `name`, and
/// where the arguments hold them its `description` and its `body`, as
/// Markdown; its result's `skill_id`, where it has one, to link to.
pub const SKILL_PRESENTATION: ToolPresentation = ToolPresentation::new("skill");

/// The skills an agent may load, listed (issue 301): the `skills` of its
/// result - each `name`, `description` and `skill_id`, to link to.
pub const SKILL_LIST_PRESENTATION: ToolPresentation = ToolPresentation::new("skill_list");

/// The tool that lists the skills the agent may load.
pub const LIST_SKILLS_TOOL_NAME: &str = "list_skills";
/// The tool that loads a skill.
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";
/// The tool that writes a skill of the agent's owner's.
pub const CREATE_SKILL_TOOL_NAME: &str = "create_skill";
/// The tool that changes a skill of the agent's owner's.
pub const UPDATE_SKILL_TOOL_NAME: &str = "update_skill";

/// The tools, asking for approval to write a skill as `requires_approval` says, their texts from
/// `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
pub(super) fn tools(
    prompts: &PromptTexts,
    requires_approval: ApprovalMode,
) -> Result<Vec<Tool>, PromptError> {
    let tool = |name: &str, parameters, requires_approval, presentation| {
        let tool = Tool {
            name: name.to_string(),
            description: String::new(),
            parameters,
            requires_approval,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation,
        };
        described(prompts, NAMESPACE, SKILLS_KIND, tool, ())
    };
    let writes = requires_approval.required(false);
    Ok(vec![
        tool(
            LIST_SKILLS_TOOL_NAME,
            ToolParameters::object(Vec::<(&str, ToolParameters)>::new(), Vec::<&str>::new()),
            false,
            SKILL_LIST_PRESENTATION,
        )?,
        // Its result names the skill; its text is given to the agent,
        // never published (issue 389).
        tool(
            LOAD_SKILL_TOOL_NAME,
            ToolParameters::object([("name", ToolParameters::string())], ["name"]),
            false,
            SKILL_PRESENTATION,
        )?,
        tool(
            CREATE_SKILL_TOOL_NAME,
            ToolParameters::object(
                [
                    ("name", ToolParameters::string()),
                    ("description", ToolParameters::string()),
                    ("body", ToolParameters::string()),
                    ("scope", ToolParameters::string()),
                ],
                ["name", "description", "body", "scope"],
            ),
            writes,
            SKILL_PRESENTATION,
        )?,
        tool(
            UPDATE_SKILL_TOOL_NAME,
            ToolParameters::object(
                [
                    ("name", ToolParameters::string()),
                    ("description", ToolParameters::string()),
                    ("body", ToolParameters::string()),
                ],
                ["name"],
            ),
            writes,
            SKILL_PRESENTATION,
        )?,
    ])
}

/// The skills a tool of `turn` may load (`reader_in`).
async fn tool_reader(turn: &Turn<'_>) -> Result<Arc<dyn SkillReader>, ToolError> {
    reader_in(&turn.situation()).await.ok_or_else(no_session)
}

/// What a tool of `turn` writes skills by (`Turn::writer`, issue 516).
async fn tool_writer(turn: &Turn<'_>) -> Result<Arc<dyn SkillWriter>, ToolError> {
    turn.writer::<dyn SkillWriter>().await.ok_or_else(no_session)
}

/// What a tool answers in a turn that reaches no session.
fn no_session() -> ToolError {
    ToolError::ExecutionFailed("the turn is in no session to load skills in".to_string())
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
            other => Err(ToolError::InvalidParameter(format!(
                "scope must be \"session\" or \"user\", not \"{other}\""
            ))),
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

/// What the model is told when reaching or writing a skill failed: what it
/// can correct, such as a name taken, text that does not fit or a skill not
/// its owner's, as such; the rest as a failure.
impl From<SkillError> for ToolError {
    fn from(error: SkillError) -> Self {
        match error {
            SkillError::Invalid(e) => ToolError::InvalidParameter(e.to_string()),
            SkillError::Store(StoreError::NameTaken { .. }) => ToolError::InvalidParameter(
                "a skill of that name exists there already; update it instead".to_string(),
            ),
            SkillError::Store(StoreError::PermissionDenied(_)) => {
                ToolError::PermissionDenied("you may write only skills of your owner's".to_string())
            }
            SkillError::NotFound(_) => {
                ToolError::InvalidParameter("there is no such skill you may change".to_string())
            }
            SkillError::NoSkillNamed(_) => ToolError::InvalidParameter(error.to_string()),
            SkillError::OwnerNotAUser => ToolError::InvalidParameter(
                "a skill belongs to a user, and your owner is none".to_string(),
            ),
            SkillError::ClassificationRefused | SkillError::NotCleared => {
                ToolError::PermissionDenied(
                    "the skill may not be classified as what you have seen".to_string(),
                )
            }
            SkillError::NotJoined => ToolError::ExecutionFailed(error.to_string()),
            SkillError::Store(e) => {
                ToolError::ExecutionFailed(format!("the skill could not be stored: {e}"))
            }
        }
    }
}

#[async_trait]
impl ToolProvider for SkillsComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        match name {
            CREATE_SKILL_TOOL_NAME => {
                let Create { name, description, body, scope } = arguments(params)?;
                let scope = SkillScope::of(&scope)?;
                Ok(format!(
                    "Write the skill \"{name}\" ({}): {description}\n\n{body}",
                    scope.told()
                ))
            }
            UPDATE_SKILL_TOOL_NAME => {
                let Update { name, description, body } = arguments(params)?;
                if description.is_none() && body.is_none() {
                    return Err(ToolError::InvalidParameter(
                        "give a new description, new steps, or both".to_string(),
                    ));
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
            _ => Ok(self
                .tools()
                .into_iter()
                .find(|t| t.name == name)
                .map(|t| t.description)
                .unwrap_or_default()),
        }
    }

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        match name {
            LIST_SKILLS_TOOL_NAME => {
                let skills = tool_reader(turn).await?.usable_skills().await?;
                let listed: Vec<Value> = skills.iter().map(|s| serde_json::json!({ "name": s.name.name.as_str(), "description": s.description.as_str(), "skill_id": s.id })).collect();
                Ok(ToolOutput::classified(
                    serde_json::json!({ "skills": listed }),
                    joined(&skills, &turn.situation().classification()),
                ))
            }
            LOAD_SKILL_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Load {
                    name: String,
                }
                let Load { name } = arguments(params)?;
                let skill = tool_reader(turn).await?.usable_skill_named(name.trim()).await?;
                // Its text is given after the step's results, as a skill a
                // user loads is (issue 389): the transcript holds every skill
                // as what it is, for a compaction to give it again. The
                // result only names it, classified as the skill, as the
                // catalogue names it.
                let given = Message {
                    id: MessageId::new(),
                    sender_id: ActorId::Agent(turn.situation().agent_id()),
                    sender_name: turn.agent_name().to_string(),
                    content: Some(
                        self.given(&skill)
                            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?,
                    ),
                    timestamp: Utc::now(),
                    ..Message::new(skill.classification.clone())
                };
                let knowledge =
                    skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
                turn.give(given, Some(knowledge));
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "loaded", "name": skill.name.name.as_str(), "skill_id": skill.id }),
                    skill.classification.clone(),
                ))
            }
            CREATE_SKILL_TOOL_NAME => {
                let Create { name, description, body, scope } = arguments(params)?;
                let skills = tool_writer(turn).await?;
                let scope = match SkillScope::of(&scope)? {
                    SkillScope::Session => {
                        Scope::SessionScope { session_id: turn.situation().session_id() }
                    }
                    SkillScope::User => skills.owner_scope().ok_or_else(|| {
                        ToolError::InvalidParameter(
                            "your owner is no user: write a skill of this session".to_string(),
                        )
                    })?,
                };
                let made = skills
                    .create(name.trim(), scope, &description, &body, turn.situation().label())
                    .await?;
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "created", "name": made.name.name.as_str(), "skill_id": made.id }),
                    turn.situation().label(),
                ))
            }
            UPDATE_SKILL_TOOL_NAME => {
                let Update { name, description, body } = arguments(params)?;
                if description.is_none() && body.is_none() {
                    return Err(ToolError::InvalidParameter(
                        "give a new description, new steps, or both".to_string(),
                    ));
                }
                let skill = tool_reader(turn).await?.usable_skill_named(name.trim()).await?;
                let skills = tool_writer(turn).await?;
                // What the turn saw goes into the skill: raised to it first,
                // so that nothing of it is ever held lower.
                let raised = skill
                    .classification
                    .join_within(&turn.situation().label(), &turn.situation().classification());
                if raised != skill.classification {
                    skills.reclassify(skill.id, raised.clone()).await?;
                }
                if let Some(description) = description {
                    skills.describe(skill.id, &description).await?;
                }
                if let Some(body) = body {
                    skills.rewrite(skill.id, &body).await?;
                }
                Ok(ToolOutput::classified(
                    serde_json::json!({ "status": "updated", "name": skill.name.name.as_str(), "skill_id": skill.id, "classification": raised.to_string() }),
                    turn.situation().label(),
                ))
            }
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}
