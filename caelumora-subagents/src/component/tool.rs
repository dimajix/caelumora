//! The tools of `subagents` (issue 558): `start_subagent`, which starts an agent of one of the
//! personalities the component is configured with on a brief, in a thread of its own at the call -
//! not waiting for it: what comes of the brief is given to the agent to read once it is there -;
//! `ask_subagent`, which gives it more to read; and `end_subagent`, which ends it. Each acts
//! through the `SubAgentWriter` the plugin binds to the turn's work (issue 516), offered where the
//! work can start a sub-agent.

use super::{listed, SubagentsComponent, SUBAGENTS_KIND};
use crate::extension::{SubAgentError, SubAgentWriter};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    arguments, described, Tool, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_domain::value::name::Name;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// The tool that starts a sub-agent.
pub const START_SUBAGENT: &str = "start_subagent";
/// The tool that gives a sub-agent more to read.
pub const ASK_SUBAGENT: &str = "ask_subagent";
/// The tool that ends a sub-agent.
pub const END_SUBAGENT: &str = "end_subagent";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    personality: String,
    brief: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    thread: ThreadId,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct End {
    thread: ThreadId,
}

/// What starting, asking or ending a sub-agent failed of, as the model is told it: what it can
/// correct (`SubAgentError::is_correctable`) is a parameter; what failed a failure; anything else -
/// what the kernel refused, without the rule, and a depth nothing the model does changes - a
/// refusal.
impl From<SubAgentError> for ToolError {
    fn from(error: SubAgentError) -> Self {
        match error {
            _ if error.is_correctable() => ToolError::InvalidParameter(error.to_string()),
            SubAgentError::Failed(_) => ToolError::ExecutionFailed(error.to_string()),
            _ => ToolError::PermissionDenied(error.to_string()),
        }
    }
}

/// The writer bound to the turn's work.
async fn writer(turn: &Turn<'_>) -> ToolResult<Arc<dyn SubAgentWriter>> {
    turn.writer::<dyn SubAgentWriter>()
        .await
        .ok_or_else(|| ToolError::ExecutionFailed("no sub-agent can be started here".to_string()))
}

/// The description of the tool `name` with `parameters`, asked for as `requires_approval` says, its
/// texts from `prompts`, rendered with `values`.
fn tool(
    prompts: &PromptTexts,
    (name, parameters): (&str, ToolParameters),
    requires_approval: bool,
    values: impl Serialize,
) -> Result<ToolDescription, PromptError> {
    let tool = ToolDescription {
        name: name.to_string(),
        description: String::new(),
        parameters,
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: ToolPresentation::default(),
    };
    described(prompts, NAMESPACE, SUBAGENTS_KIND, tool, values)
}

/// The tool `start_subagent`.
#[derive(Debug)]
pub(super) struct StartSubagentTool {
    description: ToolDescription,
    /// The personalities it may start an agent of.
    personalities: Vec<Name>,
}

impl StartSubagentTool {
    /// The tool, starting agents of `personalities`, asked for as `requires_approval` says, its
    /// texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(
        prompts: &PromptTexts,
        personalities: Vec<Name>,
        requires_approval: bool,
    ) -> Result<Self, PromptError> {
        let string = ToolParameters::string;
        let parameters = ToolParameters::object(
            [("personality", string()), ("brief", string())],
            ["personality", "brief"],
        );
        let names: Vec<&str> = personalities.iter().map(Name::as_str).collect();
        let values = context! { personalities => names, ask => ASK_SUBAGENT, end => END_SUBAGENT };
        let description = tool(prompts, (START_SUBAGENT, parameters), requires_approval, values)?;
        Ok(Self { description, personalities })
    }
}

#[async_trait]
impl Tool for StartSubagentTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    fn describe(&self, params: &Value) -> ToolResult<Option<String>> {
        let Start { personality, .. } = arguments(params)?;
        Ok(Some(format!("Start a sub-agent of '{personality}'")))
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let Start { personality, brief } = arguments(params)?;
        let known = Name::new(&personality).is_ok_and(|named| self.personalities.contains(&named));
        if !known {
            let allowed = listed(&self.personalities);
            return Err(ToolError::InvalidParameter(format!(
                "'{personality}' is none of {allowed}"
            )));
        }
        // Hung at the message whose call this is, in the thread the work happens in.
        let message = turn.calling_message().ok_or_else(|| {
            ToolError::ExecutionFailed("no message calls to hang the thread at".to_string())
        })?;
        let origin = ThreadOrigin { thread: turn.situation().thread_id(), message };
        let thread = writer(turn).await?.start(&personality, origin, brief).await?;
        Ok(json!({ "thread": thread }).into())
    }
}

/// The tool `ask_subagent`.
#[derive(Debug)]
pub(super) struct AskSubagentTool {
    description: ToolDescription,
}

impl AskSubagentTool {
    /// The tool, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let string = ToolParameters::string;
        let parameters =
            ToolParameters::object([("thread", string()), ("text", string())], ["thread", "text"]);
        Ok(Self {
            description: tool(
                prompts,
                (ASK_SUBAGENT, parameters),
                false,
                context! { start => START_SUBAGENT },
            )?,
        })
    }
}

#[async_trait]
impl Tool for AskSubagentTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let Ask { thread, text } = arguments(params)?;
        writer(turn).await?.ask(thread, text).await?;
        Ok(json!({ "asked": thread }).into())
    }
}

/// The tool `end_subagent`.
#[derive(Debug)]
pub(super) struct EndSubagentTool {
    description: ToolDescription,
}

impl EndSubagentTool {
    /// The tool, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object([("thread", ToolParameters::string())], ["thread"]);
        Ok(Self {
            description: tool(
                prompts,
                (END_SUBAGENT, parameters),
                false,
                context! { start => START_SUBAGENT },
            )?,
        })
    }
}

#[async_trait]
impl Tool for EndSubagentTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let End { thread } = arguments(params)?;
        writer(turn).await?.end(thread).await?;
        Ok(json!({ "ended": thread }).into())
    }
}

#[async_trait]
impl ToolProvider for SubagentsComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }

    /// Its tools where the work can start a sub-agent: where its session runs (issue 558).
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Arc<dyn Tool>>, String> {
        if !situation.has_writer::<dyn SubAgentWriter>().await {
            return Err("no sub-agent can be started here".to_string());
        }
        Ok(self.tools())
    }
}
