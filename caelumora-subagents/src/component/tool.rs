//! The tools of `subagents` (issue 558): `start_subagent`, which starts an agent of one of the
//! personalities the component is configured with on a brief, in a thread of its own at the call -
//! not waiting for it: what comes of the brief is given to the agent to read once it is there -;
//! `ask_subagent`, which gives it more to read; and `end_subagent`, which ends it. Each acts
//! through the `SubAgentWriter` the plugin binds to the turn's work (issue 516), offered where the
//! work can start a sub-agent.

use super::{listed, SubagentsComponent, SUBAGENTS_KIND};
use crate::extension::{ErrorClass, SubAgentError, SubAgentWriter};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    arguments, described, tool_text, Tool, ToolError, ToolOutput, ToolProvider, ToolResult,
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

/// The thread a call of `start_subagent` or `ask_subagent` gave a brief into, as its result names
/// it - beside what it tells of what it did. Read from what the model was given, which is the
/// result itself as long as the tools are shown and do not classify what they give
/// (`successful_result`): so `tool` makes them, and so they answer.
#[derive(Deserialize)]
struct Briefed {
    thread: ThreadId,
}

/// Whether `turn` gave the sub-agent of `thread` a brief already (issue 616): a call of
/// `start_subagent` or `ask_subagent` naming it, which succeeded.
fn briefed_in(turn: &Turn<'_>, thread: ThreadId) -> bool {
    turn.tool_results().iter().any(|result| {
        [START_SUBAGENT, ASK_SUBAGENT].contains(&result.tool_name.as_str())
            && result.status.is_ok()
            && serde_json::from_str::<Briefed>(&result.content).is_ok_and(|b| b.thread == thread)
    })
}

/// What starting, asking or ending a sub-agent failed of, as the model is told it, by its class
/// (`SubAgentError::class`): what it can correct is a parameter, a refusal a refusal, what failed a
/// failure.
impl From<SubAgentError> for ToolError {
    fn from(error: SubAgentError) -> Self {
        let told = error.to_string();
        match error.class() {
            ErrorClass::Correctable => ToolError::InvalidParameter(told),
            ErrorClass::Refused => ToolError::PermissionDenied(told),
            ErrorClass::Failed => ToolError::ExecutionFailed(told),
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
    /// What its result tells of what it did: that the sub-agent works on the brief and nothing
    /// more is needed until its answer (issue 616).
    started: String,
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
        let started = tool_text(prompts, NAMESPACE, SUBAGENTS_KIND, "started", ())?;
        Ok(Self { description, personalities, started })
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
        Ok(json!({ "thread": thread, "started": self.started }).into())
    }
}

/// The tool `ask_subagent`.
#[derive(Debug)]
pub(super) struct AskSubagentTool {
    description: ToolDescription,
    /// What its result tells of what it did: that the sub-agent is given the text once it answered
    /// what it was given before, and nothing more is needed until its answer (issue 616).
    asked: String,
    /// Why a call in the turn that gave the sub-agent a brief is refused: its answer cannot come
    /// before the turn ends (issue 616).
    briefed_already: String,
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
        let values = context! { start => START_SUBAGENT };
        let description = tool(prompts, (ASK_SUBAGENT, parameters), false, values)?;
        let asked = tool_text(prompts, NAMESPACE, SUBAGENTS_KIND, "asked", ())?;
        let briefed_already = tool_text(prompts, NAMESPACE, SUBAGENTS_KIND, "briefed_already", ())?;
        Ok(Self { description, asked, briefed_already })
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
        // Never answered within the turn: the answer is given to read between turns.
        if briefed_in(turn, thread) {
            return Err(ToolError::ExecutionFailed(self.briefed_already.clone()));
        }
        writer(turn).await?.ask(thread, text).await?;
        Ok(json!({ "thread": thread, "asked": self.asked }).into())
    }
}

/// The tool `end_subagent`.
#[derive(Debug)]
pub(super) struct EndSubagentTool {
    description: ToolDescription,
    /// What its result tells of what it did: that the sub-agent is let go and its thread closed
    /// (issue 616).
    ended: String,
}

impl EndSubagentTool {
    /// The tool, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object([("thread", ToolParameters::string())], ["thread"]);
        let values = context! { start => START_SUBAGENT };
        let description = tool(prompts, (END_SUBAGENT, parameters), false, values)?;
        let ended = tool_text(prompts, NAMESPACE, SUBAGENTS_KIND, "ended", ())?;
        Ok(Self { description, ended })
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
        Ok(json!({ "thread": thread, "ended": self.ended }).into())
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
