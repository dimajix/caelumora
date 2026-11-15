//! The tools of `skills` (issues 22, 281, 301, 584): those that list and load the skills the agent
//! may load, and write and change those of its owner's, each a type of its own.
//!
//! - **`list_skills`** gives the catalogue as a tool, for a model whose prompt was built before a
//!   skill appeared - each skill with its id, for the channel to link to (issue 301);
//!   **`load_skill`** names the skill in its result, classified as the skill, and gives its text
//!   after the step's results, as `/skill` does (issue 389): the channel sees only which skill was
//!   loaded (issue 281), its history records the text as what the agent was given, gone to nobody
//!   (issue 417), and it is not given again where it stands so already, which the result says
//!   (`given above`, issue 541).
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
    arguments, described, tool_text, ApprovalMode, FixedTools, Tool, ToolError, ToolOutput,
    ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::actor::ActorId;
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::prompt_catalogue::PromptError;
use caelumora_domain::question::Answers;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::StoreError;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolPresentation, ToolVisibility};
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

/// The tools of `skills`, asking for approval to write a skill as `requires_approval` says, their
/// texts from its prompts.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
pub(super) fn tools(
    skills: &Arc<SkillsComponent>,
    requires_approval: ApprovalMode,
) -> Result<FixedTools, PromptError> {
    let prompts = &skills.prompts;
    let tool = |name: &str, parameters, requires_approval, presentation| {
        let tool = ToolDescription {
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
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ListSkillsTool {
            description: tool(
                LIST_SKILLS_TOOL_NAME,
                ToolParameters::object(Vec::<(&str, ToolParameters)>::new(), Vec::<&str>::new()),
                false,
                SKILL_LIST_PRESENTATION,
            )?,
        }),
        Arc::new(LoadSkillTool {
            description: tool(
                LOAD_SKILL_TOOL_NAME,
                ToolParameters::object([("name", ToolParameters::string())], ["name"]),
                false,
                SKILL_PRESENTATION,
            )?,
            skills: Arc::clone(skills),
        }),
        Arc::new(CreateSkillTool {
            description: tool(
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
        }),
        Arc::new(UpdateSkillTool {
            description: tool(
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
        }),
    ];
    let description = tool_text(prompts, NAMESPACE, SKILLS_KIND, "description", ())?;
    Ok(FixedTools::new(description, tools))
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

/// The tool `list_skills`: the skills the agent may load.
#[derive(Debug)]
struct ListSkillsTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for ListSkillsTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        _params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let skills = tool_reader(turn).await?.usable_skills().await?;
        let listed: Vec<Value> = skills.iter().map(|s| serde_json::json!({ "name": s.name.name.as_str(), "description": s.description.as_str(), "skill_id": s.id })).collect();
        Ok(ToolOutput::classified(
            serde_json::json!({ "skills": listed }),
            joined(&skills, &turn.situation().classification()),
        ))
    }
}

/// The tool `load_skill`: a skill given to the agent, named in its result.
#[derive(Debug)]
struct LoadSkillTool {
    description: ToolDescription,
    /// The component, which renders a skill as it is given (`SkillsComponent::given`).
    skills: Arc<SkillsComponent>,
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct Load {
            name: String,
        }
        let Load { name } = arguments(params)?;
        let skill = tool_reader(turn).await?.usable_skill_named(name.trim()).await?;
        // Its text is given after the step's results, as a skill a user loads is (issue
        // 389): the transcript holds every skill as what it is, for a compaction to give it
        // again - unless it stands there as it is already (issue 541). The result only
        // names it, classified as the skill, as the catalogue names it.
        let given = Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(turn.situation().agent_id()),
            sender_name: turn.agent_name().to_string(),
            content: Some(
                self.skills.given(&skill).map_err(|e| ToolError::ExecutionFailed(e.to_string()))?,
            ),
            timestamp: Utc::now(),
            ..Message::new(skill.classification.clone())
        };
        let knowledge =
            skill.knowledge(turn.situation().channel_id(), turn.situation().session_id());
        let status = if turn.give(given, Some(knowledge)) { "loaded" } else { "given above" };
        Ok(ToolOutput::classified(
            serde_json::json!({ "status": status, "name": skill.name.name.as_str(), "skill_id": skill.id }),
            skill.classification.clone(),
        ))
    }
}

/// The tool `create_skill`: a skill of the agent's owner's, written.
#[derive(Debug)]
struct CreateSkillTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for CreateSkillTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
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
        Ok(ToolOutput::of_turn(
            serde_json::json!({ "status": "created", "name": made.name.name.as_str(), "skill_id": made.id }),
            turn,
        ))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        let Create { name, description, body, scope } = arguments(params)?;
        let scope = SkillScope::of(&scope)?;
        Ok(format!("Write the skill \"{name}\" ({}): {description}\n\n{body}", scope.told()))
    }
}

/// The tool `update_skill`: a skill of the agent's owner's, changed.
#[derive(Debug)]
struct UpdateSkillTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for UpdateSkillTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
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
        Ok(ToolOutput::of_turn(
            serde_json::json!({ "status": "updated", "name": skill.name.name.as_str(), "skill_id": skill.id, "classification": raised.to_string() }),
            turn,
        ))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
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
}
