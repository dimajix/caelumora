//! Tools (issue 52): the facets `ToolProvider` and `ToolExecutor`, the
//! `GuardedToolExecutor` the runtime puts in front of an agent's executor
//! (issue 127), and the built-in tool components, one file per component
//! kind. A component that offers tools along with another facet - a prompt
//! part, say - stays in `component_impl`.

pub mod ask_user_tool;
pub mod calculator_tool;
pub mod default_tool_executor;
pub mod echo_tool;
pub mod mcp_tool;
pub mod sleep_tool;
pub mod stop_tool;
pub mod terminal_tool;

use crate::agent::turn::Turn;
use crate::component::{ComponentImpl, ComponentRegistry};
use crate::controller::{ChannelController, SessionController};
use async_trait::async_trait;
use caelumora_model::approval::{ApprovalRule, RuleDecision, ToolMatcher};
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::question::{Answers, Questions};
use crate::runtime::ChannelHandle;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolCallState, ToolSet, ToolVisibility, ToolResultStatus};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info, warn};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
}

pub type ToolResult<T> = Result<T, ToolError>;

/// What a tool gives back (issue 133): its value and, if the tool
/// classifies it - by its input, by what it read, by the source it drew
/// on - how sensitive it is. `None` leaves it to the tool's configuration
/// or the channel (`GuardedToolExecutor`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub classification: Option<Classification>,
}

impl ToolOutput {
    /// `value`, classified `classification` by the tool.
    pub fn classified(value: Value, classification: Classification) -> Self {
        Self { value, classification: Some(classification) }
    }
}

/// A value the tool does not classify.
impl From<Value> for ToolOutput {
    fn from(value: Value) -> Self {
        Self { value, classification: None }
    }
}

/// The arguments of a tool call read as `T`, the struct that names the
/// tool's parameters - the one way built-in tools read their parameters
/// (issue 111). Arguments that do not fit are the model's to correct:
/// `InvalidParameter` with serde's account of what is wrong ("missing field
/// `text`", "invalid type: …"). No arguments at all read as an empty object,
/// so that a tool whose parameters are all optional accepts them.
pub fn arguments<T: DeserializeOwned>(params: &Value) -> ToolResult<T> {
    let empty = Value::Object(Default::default());
    let params = if params.is_null() { &empty } else { params };
    T::deserialize(params).map_err(|e| ToolError::InvalidParameter(e.to_string()))
}

/// The parameter `id` of a tool that takes the number of an entry, e.g. a
/// note to revoke.
pub fn id_argument<T: DeserializeOwned>(params: &Value) -> ToolResult<T> {
    #[derive(Deserialize)]
    struct Id<T> { id: T }
    let Id { id } = arguments(params)?;
    Ok(id)
}

/// The parameters `level` and `compartments` of a tool that reclassifies
/// something (issue 133), as a classification. One that cannot be - a
/// public one with compartments - is the model's to correct.
pub fn classification_argument(params: &Value) -> ToolResult<Classification> {
    #[derive(Deserialize)]
    struct Asked { level: caelumora_model::confidentiality::Confidentiality, #[serde(default)] compartments: Vec<String> }
    let Asked { level, compartments } = arguments(params)?;
    Classification::new(level, compartments.into_iter().map(caelumora_model::confidentiality::Compartment::new))
        .map_err(|e| ToolError::InvalidParameter(e.to_string()))
}

/// The parameters of a tool that reclassifies the entry numbered `id`,
/// described as `what` (e.g. "your note"): `id`, `level` and `compartments`.
pub fn reclassify_parameters(what: &str) -> caelumora_model::tool::ToolParameters {
    use caelumora_model::tool::ToolParameters;
    ToolParameters::object(
        [
            ("id", ToolParameters::number(format!("The number of {what}"))),
            ("level", ToolParameters::string("The new level: public, internal, confidential or strictly_confidential")),
            ("compartments", ToolParameters::array(ToolParameters::string("A compartment"), "Who it is meant for; none: everybody of the level. Public takes none.")),
        ],
        ["id", "level"],
    )
}

/// The parameter `text` of a tool that notes or remembers something,
/// trimmed; an empty one is no text.
pub fn text_argument(params: &Value) -> ToolResult<String> {
    #[derive(Deserialize)]
    struct Text { text: String }
    let Text { text } = arguments(params)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(ToolError::InvalidParameter("'text' must not be empty".to_string()));
    }
    Ok(text.to_string())
}

/// What the model is given as the result of a tool call that produced none
/// (issue 15): a small JSON object with the `status` and a `message` that
/// says what happened and what to do next, e.g.
/// `{"status":"denied","message":"The user declined ..."}`. Successful
/// results are passed on unchanged.
fn unsuccessful_result(status: ToolResultStatus, message: &str) -> String {
    serde_json::json!({ "status": status, "message": message }).to_string()
}

/// What the model is given as the result of a call that succeeded: the
/// tool's `value` - marked as the agent's alone if the channel does not see
/// it (issue 66), i.e. if the call's content is its owner's
/// (`Visibility::Owner`). The answer text is always published, so a result
/// the channel does not see is worthless redacted if the model quotes it.
pub fn successful_result(value: Value, visibility: ToolVisibility) -> String {
    if visibility.content.is_channel() {
        return value.to_string();
    }
    serde_json::json!({
        "result": value,
        "note": "Only you see this result - the channel does not. Do not quote or reveal it in your answers.",
    }).to_string()
}

/// The result for a call a user declined.
pub(crate) fn denied_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Denied,
        "The user declined this tool call. Do not repeat it; continue without it or ask the user how to proceed.",
    )
}

/// The result for a call nobody approved in time.
pub(crate) fn timed_out_result() -> String {
    unsuccessful_result(
        ToolResultStatus::TimedOut,
        "No one approved this tool call in time; it was not executed. You may ask the user whether to try again.",
    )
}

/// The result for a call whose questions a person declined to answer
/// (issue 165), by saying so or by writing a message instead.
pub(crate) fn declined_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Declined,
        "The user declined to answer the questions of this tool call; it was not executed. Read what they wrote instead, if anything, and go on without the answers or ask differently.",
    )
}

/// The result for a call that was approved but has no result when its
/// agent is loaded again (issue 75): the server stopped before the call
/// ran, or before its result was stored. The runtime cannot tell which, so
/// it never runs the call again - the model decides.
pub(crate) fn not_executed_result() -> String {
    unsuccessful_result(
        ToolResultStatus::NotExecuted,
        "This tool call was not executed: the server restarted before it ran. Decide again whether to call it; if its effect matters, check first.",
    )
}

/// The result for a call that was running when its turn was cancelled or
/// stopped (issue 161): it was interrupted, and what it did before stays
/// done. The runtime cannot tell how far it got - the model checks before
/// it calls it again.
pub(crate) fn interrupted_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Interrupted,
        "This tool call was interrupted while it ran, because the turn ended. It may have taken effect in part or in full. Check its effect before calling it again.",
    )
}

/// The result for a call that had no result when its turn ended (issue
/// 148): it never ran, `because` - why the turn ended, as in "the turn
/// ended before it ran, because it was cancelled".
pub(crate) fn abandoned_result(because: &str) -> String {
    unsuccessful_result(
        ToolResultStatus::NotExecuted,
        &format!("This tool call was not executed: the turn ended before it ran, because {because}. Decide again whether to call it."),
    )
}

/// The result for a call whose result is classified `classification`, which
/// may not flow into the channel (issue 133). The model is told the
/// classification, so that it can tell the user why, and not to look for
/// the result elsewhere.
pub(crate) fn withheld_result(classification: &Classification) -> String {
    unsuccessful_result(
        ToolResultStatus::Withheld,
        &format!("The result of this tool call is classified {classification}, which may not be shown in this channel; it was withheld. Do not try to obtain it another way; tell the user if it matters."),
    )
}

/// The result for a call the policy does not permit. Deliberately without the
/// reason: telling the model why describes the policy to something that may
/// try to route around it. The reason is in the log.
pub(crate) fn forbidden_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Forbidden,
        "This tool call is not permitted. Do not retry it; continue without it or tell the user that you cannot do this.",
    )
}

impl ToolError {
    /// The status and the model-facing result for a call that failed with
    /// this error. `available` names the tools that do exist, offered when
    /// the model called one that does not.
    pub fn to_result(&self, available: &[String]) -> (ToolResultStatus, String) {
        let (status, message) = match self {
            ToolError::PermissionDenied(_) => return (ToolResultStatus::Forbidden, forbidden_result()),
            ToolError::UnknownTool(name) => (
                ToolResultStatus::UnknownTool,
                format!("There is no tool named '{}'. Available tools: {}.", name, if available.is_empty() { "none".to_string() } else { available.join(", ") }),
            ),
            ToolError::InvalidParameter(reason) => (
                ToolResultStatus::InvalidArguments,
                format!("The arguments were invalid: {}. Correct them and call the tool again.", reason),
            ),
            ToolError::ExecutionFailed(reason) => (
                ToolResultStatus::Failed,
                format!("The tool failed: {}", reason),
            ),
        };
        (status, unsuccessful_result(status, &message))
    }
}

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// What its tools are for, written for a model: the description of its
    /// tool set, which is named after its component (issue 194).
    fn description(&self) -> String;
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;
    /// Why it gives no tools now, in general terms - never the error, which
    /// may name hosts (issue 32). `None`: available.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// Where the arguments of its calls go (issue 180): the classification
    /// its remote side is cleared for. By default none - they stay in the
    /// system; a provider that sends them out says where.
    fn destination(&self) -> Option<Classification> {
        None
    }

    /// Executes a specific tool by name - with the `answers` to the
    /// questions it asked for the call (`questions`), if it asked any.
    async fn execute(&self, name: &str, params: &Value, answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput>;
    /// What the call of tool `name` with `params` asks a person before it
    /// runs (issue 165): the answers are handed to `execute`, and reach the
    /// model only as far as the tool puts them into its result. By default
    /// nothing; an error is the call's, as with `describe`.
    fn questions(&self, _name: &str, _params: &Value) -> ToolResult<Option<Questions>> {
        Ok(None)
    }
    /// What other calls are like the call of tool `name` with `params`
    /// (issue 13): the matcher a rule derived from it is offered with. By
    /// default the calls exactly like it; a tool whose argument is a
    /// language says better what makes two calls alike.
    fn matcher(&self, name: &str, params: &Value) -> ToolMatcher {
        ToolMatcher::exact(&ToolCall { id: None, function_name: name.to_string(), arguments: params.clone(), visibility: ToolVisibility::SHOWN, presentation: Default::default() })
    }
    /// Provides a textual description for a specific tool call
    fn describe(&self,  name: &str, _params: &Value) -> ToolResult<String> {
        let tools = self.tools();
        if let Some(tool) = tools.iter().find(|t| t.name == name) {
            Ok(tool.description.to_string())
        }
        else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}


/// How an agent's tool calls are run - a component facet of which an agent
/// has exactly one (issue 127): the component of its personality that
/// provides it, or the default `default_tool_executor`, unless the
/// configuration names another (`AppConfig::component_defaults`). It decides
/// how a tool is run, never whether, and not what the model is told: the
/// runtime checks the permission before it delegates, and makes the result
/// ([`GuardedToolExecutor`]), so that no configured component can run a
/// call the policy refuses or hand the model what it may not see.
#[async_trait]
pub trait ToolExecutor: Send + Sync + Debug {
    /// Runs `call` with the tool that provides it: what the tool gave, or
    /// what went wrong (`ToolError::UnknownTool` if no tool has the name).
    /// Called only for calls the policy permits, with the `answers` to the
    /// questions the tool asked for the call.
    async fn execute(&self, call: &ToolCall, answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput>;
    fn describe(&self, call: &ToolCall) -> ToolResult<String>;
    /// What `call` asks a person before it runs, as its tool says (issue
    /// 165). No default: an executor that left it out would run a tool
    /// without the answers it needs.
    fn questions(&self, call: &ToolCall) -> ToolResult<Option<Questions>>;
    /// What other calls are like `call`, as its tool says (issue 13); by
    /// default the calls exactly like it.
    fn matcher(&self, call: &ToolCall) -> ToolMatcher {
        ToolMatcher::exact(call)
    }
    /// The tools there are now, in the sets of the providers they come from
    /// (issue 194), for the model and for labelling its calls. Asked of the
    /// providers each time: a provider's tools may change while the agent
    /// runs - an MCP server replaced by a newer one - and caching them is
    /// the provider's (issue 100).
    fn tool_sets(&self) -> Vec<ToolSet>;
}

/// How a tool call of the model is admitted, before anything runs
/// (`GuardedToolExecutor::admit`, issue 155).
#[derive(Debug)]
pub enum Admission {
    /// A user decides first; they are shown `description`. `explicit` if a
    /// rule of a place asked for it: a decision of their own (issue 13).
    /// `suggested` is what a rule derived from the call would match.
    /// Approved, it asks `questions`, if it has any (issue 165).
    Ask { description: String, explicit: bool, suggested: ToolMatcher, questions: Option<Questions> },
    /// The users answer its questions first (issue 165), as put to them.
    Await(Questions),
    /// It runs as it is.
    Run,
    /// It cannot succeed and is answered at once, without asking anyone.
    Answered(ToolCallResult),
}

impl Admission {
    /// The state `call` starts with, admitted so.
    pub fn state(self, call: ToolCall) -> ToolCallState {
        match self {
            Admission::Ask { description, explicit, suggested, questions } => ToolCallState::pending(call, description, explicit, suggested, questions),
            Admission::Await(questions) => ToolCallState::asking(call, questions),
            Admission::Run => ToolCallState::approved(call),
            Admission::Answered(result) => ToolCallState::answered(call, result),
        }
    }
}

/// Which calls of a provider's tools require human approval, as configured
/// in its `requires_approval`: `true` (all of them, the default), `false`
/// (none), or `"unless_read_only"` - all but those of the tools that only
/// read (issue 246), as the provider tells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "Value")]
pub(crate) enum ApprovalMode {
    #[default]
    Always,
    Never,
    UnlessReadOnly,
}

impl ApprovalMode {
    /// Whether calls of a tool require approval, which only reads if
    /// `read_only`.
    pub(crate) fn required(self, read_only: bool) -> bool {
        match self {
            ApprovalMode::Always => true,
            ApprovalMode::Never => false,
            ApprovalMode::UnlessReadOnly => !read_only,
        }
    }
}

impl TryFrom<Value> for ApprovalMode {
    type Error = &'static str;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Bool(true) => Ok(ApprovalMode::Always),
            Value::Bool(false) => Ok(ApprovalMode::Never),
            Value::String(text) if text == "unless_read_only" => Ok(ApprovalMode::UnlessReadOnly),
            _ => Err("requires_approval must be true, false or \"unless_read_only\""),
        }
    }
}

/// Whether `call` names a tool `turn` was offered (issue 180): a tool of a
/// set withheld in the turn's channel, or of none, is not called by name.
fn offered(call: &ToolCall, turn: &Turn<'_>) -> bool {
    turn.tools().iter().any(|tool| tool.name == call.function_name)
}

/// An agent's [`ToolExecutor`] with what the runtime keeps to itself
/// (issues 127, 133): only an agent calls tools, and only what the policy
/// permits (`CallTool`) - asked before a call is put up for approval
/// (`admit`, which also decides whether a user is asked) and again when
/// it is run. And the result the model is
/// given is made here, the one place: what the tool gave, or what went
/// wrong and what to do next (issue 15), classified - as the tool
/// classified it, else as the tool is configured, else as the channel -
/// and withheld if that may not flow into the channel, before the model
/// sees it. Whatever executor a personality configures, it only ever sees
/// permitted calls, and never decides what the model is told.
///
/// The rules of the places the agent works in (issue 13) - the system's,
/// its session's, its channel's - are applied here too, as they stand at
/// each call: a rule that denies refuses a call as the policy does, before
/// anyone is asked and again when it is run, and a rule that asks puts the
/// call to a person for a decision of their own, whatever its tool says.
#[derive(Debug)]
pub struct GuardedToolExecutor {
    security_manager: Arc<dyn SecurityManager>,
    executor: ComponentImpl<dyn ToolExecutor>,
    system_rules: Arc<[ApprovalRule]>,
    session: Arc<SessionController>,
    channel: Arc<ChannelController>,
    /// The channel as it runs, whose users a call's questions are put to.
    running: ChannelHandle,
}

impl GuardedToolExecutor {
    /// `executor`, guarded by `security_manager` and by the rules of the
    /// system (`system_rules`, `AppConfig::approvals`), of `session` and of
    /// `channel`, where the agent works - which, `running`, puts the
    /// questions of its calls to its users.
    pub(crate) fn new(security_manager: Arc<dyn SecurityManager>, executor: ComponentImpl<dyn ToolExecutor>, system_rules: Arc<[ApprovalRule]>, session: Arc<SessionController>, channel: Arc<ChannelController>, running: ChannelHandle) -> Self {
        Self { security_manager, executor, system_rules, session, channel, running }
    }

    /// What `call` asks before it runs, as its tool says, checked and put to
    /// the users of the channel (issue 165) - the agent's owner being whom
    /// `turn` acts for. Every error is the call's: it cannot be asked.
    fn questions(&self, call: &ToolCall, turn: &Turn<'_>) -> ToolResult<Option<Questions>> {
        let Some(questions) = self.executor.questions(call)? else { return Ok(None) };
        questions.check().map_err(|e| ToolError::InvalidParameter(e.to_string()))?;
        let channel = self.running.get().ok_or_else(|| ToolError::ExecutionFailed("the channel does not run".to_string()))?;
        channel.put(questions, turn.security_context().initiator_id)
            .map(Some)
            .map_err(|e| ToolError::InvalidParameter(e.to_string()))
    }

    /// The rule of a place that decides `call` now, if one does: the
    /// strongest of the system's, the session's and the channel's
    /// (`ApprovalRule::decide`).
    fn place_rule(&self, call: &ToolCall) -> Option<ApprovalRule> {
        let (session, channel) = (self.session.load(), self.channel.load());
        let rules = self.system_rules.iter().chain(&session.approval_rules).chain(&channel.approval_rules);
        ApprovalRule::decide(rules, call, session.id, channel.id, chrono::Utc::now()).cloned()
    }

    /// Runs `call` if it may be run now, with the `answers` to the questions
    /// its tool asked, and answers it either way. Once
    /// `cancelled` - the turn is cancelled or stopped - the call is no
    /// longer waited for (issue 161): whatever the executor and its tool
    /// are doing is dropped, and the call is answered as interrupted, with
    /// when it started, so that it counts as having run. Dropping a tool
    /// does not undo what it did (issue 20).
    pub async fn execute(&self, call: &ToolCall, answers: Option<&Answers>, turn: &mut Turn<'_>, cancelled: &CancellationToken) -> ToolCallResult {
        if let Err(e) = self.check(call, turn).await {
            return self.failed(call, e, turn);
        }
        let started_at = chrono::Utc::now();
        // A tool that is done when the turn ends has its result.
        let outcome = tokio::select! {
            biased;
            outcome = self.executor.execute(call, answers, turn) => Some(outcome),
            () = cancelled.cancelled() => None,
        };
        let finished_at = chrono::Utc::now();
        let Some(outcome) = outcome else {
            info!(tool = %call.function_name, duration_ms = (finished_at - started_at).num_milliseconds(), "Tool interrupted: its turn ended");
            // A fixed text of the runtime's: as the turn (issue 168).
            let mut result = ToolCallResult::of(call, ToolResultStatus::Interrupted, interrupted_result(), turn.label());
            result.started_at = Some(started_at);
            result.finished_at = Some(finished_at);
            return result;
        };
        // A call of a tool that does not exist never ran: no timing.
        if matches!(outcome, Err(ToolError::UnknownTool(_))) {
            return self.failed(call, ToolError::UnknownTool(call.function_name.clone()), turn);
        }
        // Session, channel and agent are on the turn's span.
        debug!(tool = %call.function_name, duration_ms = (finished_at - started_at).num_milliseconds(), ok = outcome.is_ok(), "Tool ran");
        let mut result = match outcome {
            Ok(output) => self.admitted(call, output, turn).await,
            Err(e) => {
                warn!("Tool call '{}' failed: {}", call.function_name, e);
                self.failed(call, e, turn)
            }
        };
        result.started_at = Some(started_at);
        result.finished_at = Some(finished_at);
        result
    }

    /// The result for `call`, which failed with `error`: what went wrong and
    /// what to do next, classified as the channel.
    fn failed(&self, call: &ToolCall, error: ToolError, turn: &Turn<'_>) -> ToolCallResult {
        let available: Vec<String> = turn.tools().iter().map(|t| t.name.clone()).collect();
        let (status, content) = error.to_result(&available);
        ToolCallResult::of(call, status, content, turn.classification())
    }

    /// The result for `call`, whose tool gave `output`: classified as the
    /// tool classified it, else as the tool is configured - as the turn
    /// began with its tools - else as the channel, and withheld if that may
    /// not flow into the channel (`AccessQuestions::may_publish`, in the agent's
    /// name: the result enters its context in the channel).
    async fn admitted(&self, call: &ToolCall, output: ToolOutput, turn: &Turn<'_>) -> ToolCallResult {
        let channel = turn.classification();
        let configured = turn.tools().iter().find(|t| t.name == call.function_name).and_then(|t| t.classification.clone());
        let classification = output.classification.or(configured).unwrap_or_else(|| channel.clone());
        let context = turn.security_context();
        if !self.security_manager.may_publish(context, context.caller_id, &classification, &channel).await {
            debug!(tool = %call.function_name, %classification, "Withheld a result the channel does not admit");
            return ToolCallResult::of(call, ToolResultStatus::Withheld, withheld_result(&classification), channel);
        }
        ToolCallResult::of(call, ToolResultStatus::Ok, successful_result(output.value, call.visibility), classification)
    }

    /// How `call`, made by the model in `turn`, is admitted: a call that
    /// cannot succeed is answered at once rather than put to a user who
    /// could only approve it in vain - in the order `execute` would find
    /// out: first the policy (`CallTool`) and a rule of a place that denies
    /// it (issue 13; `execute` checks both again), then the call itself
    /// (issue 118) and the questions it asks (issue 165). Otherwise a user is
    /// asked if a rule of a place asks for it - explicitly, for a decision
    /// of their own - or if its tool, as the turn began with it (issue 137),
    /// requires approval, and shown what the tool says of the call; the
    /// questions are asked once it is approved, or at once.
    pub async fn admit(&self, call: &ToolCall, turn: &Turn<'_>) -> Admission {
        let agent = turn.agent_id();
        if !offered(call, turn) {
            info!("Tool call '{}' by agent {} names a tool it was not offered", call.function_name, agent);
            return Admission::Answered(self.failed(call, ToolError::UnknownTool(call.function_name.clone()), turn));
        }
        if !self.security_manager.may_call_tool(turn.security_context(), call).await {
            info!("Tool call '{}' by agent {} forbidden by the security manager", call.function_name, agent);
            return Admission::Answered(ToolCallResult::of(call, ToolResultStatus::Forbidden, forbidden_result(), turn.label()));
        }
        let rule = self.place_rule(call);
        if let Some(rule) = rule.as_ref().filter(|r| r.decision == RuleDecision::Deny) {
            info!(rule = %rule.id, "Tool call '{}' by agent {} denied by a rule", call.function_name, agent);
            return Admission::Answered(ToolCallResult::of(call, ToolResultStatus::Forbidden, forbidden_result(), turn.label()));
        }
        let description = match self.executor.describe(call) {
            Ok(description) => description,
            Err(e @ (ToolError::InvalidParameter(_) | ToolError::UnknownTool(_))) => {
                info!("Tool call '{}' by agent {} cannot run: {}", call.function_name, agent, e);
                return Admission::Answered(self.failed(call, e, turn));
            }
            Err(e) => {
                warn!("Cannot describe tool call '{}' for approval: {}", call.function_name, e);
                "No description".to_string()
            }
        };
        let questions = match self.questions(call, turn) {
            Ok(questions) => questions,
            Err(e) => {
                info!("Tool call '{}' by agent {} cannot ask its questions: {}", call.function_name, agent, e);
                return Admission::Answered(self.failed(call, e, turn));
            }
        };
        let explicit = rule.is_some_and(|r| r.decision == RuleDecision::Ask);
        let asks = explicit || turn.tools().iter().any(|t| t.name == call.function_name && t.requires_approval);
        match questions {
            _ if asks => Admission::Ask { description, explicit, suggested: self.executor.matcher(call), questions },
            Some(questions) => Admission::Await(questions),
            None => Admission::Run,
        }
    }

    /// The tool sets the agent is offered in a channel classified
    /// `channel`, acting in `context` (issue 180): a set whose destination
    /// the channel may not flow into (`AccessQuestions::may_publish`, in the
    /// agent's name) is offered as unavailable, without its tools - its
    /// arguments would carry what the channel holds out of the system.
    pub async fn tool_sets(&self, context: &SecurityContext, channel: &Classification) -> Vec<ToolSet> {
        let mut offered = Vec::new();
        for set in self.executor.tool_sets() {
            let withheld = match (&set.destination, &set.unavailable) {
                (Some(destination), None) => !self.security_manager.may_publish(context, context.caller_id, channel, destination).await,
                _ => false,
            };
            if withheld {
                debug!(set = %set.name, %channel, "Withheld a tool set whose remote side is not cleared for the channel");
            }
            offered.push(if withheld { set.withheld() } else { set });
        }
        offered
    }

    /// Nothing if `call` may be run now: the caller is an agent, the policy
    /// permits the call and no rule of a place denies it.
    async fn check(&self, call: &ToolCall, turn: &Turn<'_>) -> ToolResult<()> {
        if !offered(call, turn) {
            return Err(ToolError::UnknownTool(call.function_name.clone()));
        }
        let security = turn.security_context();
        if security.caller_id.as_agent_id().is_none() {
            return Err(ToolError::PermissionDenied(format!("tools can only be called by agents, not by {}", security.caller_id)));
        }
        self.security_manager.require(security, ActionType::CallTool(call), || ToolError::PermissionDenied(call.function_name.clone())).await?;
        // A rule that denies may have been set while the call waited.
        match self.place_rule(call) {
            Some(rule) if rule.decision == RuleDecision::Deny => Err(ToolError::PermissionDenied(format!("rule {}", rule.id))),
            _ => Ok(()),
        }
    }
}

/// Registers the tool kinds with `registry`, and the `ToolExecutor` as a
/// facet of which an agent has exactly one, with its default kind.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn ToolExecutor>(Some(default_tool_executor::DEFAULT_TOOL_EXECUTOR_KIND));

    ask_user_tool::register(registry);
    calculator_tool::register(registry);
    default_tool_executor::register(registry);
    echo_tool::register(registry);
    mcp_tool::register(registry);
    sleep_tool::register(registry);
    stop_tool::register(registry);
    terminal_tool::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::default_tool_executor::DefaultToolExecutor;
    use crate::agent::turn::Turn;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};

    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::tool::ToolResultStatus;
    use serde_json::json;

    #[test]
    fn arguments_are_read_as_a_struct_and_what_does_not_fit_is_the_models_to_correct() {
        #[derive(Deserialize, Debug)]
        struct Note { text: String, #[serde(default)] channel: Option<String> }
        #[derive(Deserialize, Debug)]
        struct Optional { #[serde(default)] channel: Option<String> }

        let note: Note = arguments(&json!({ "text": "hello", "extra": 1 })).expect("unknown fields are ignored");
        assert_eq!((note.text.as_str(), note.channel), ("hello", None));
        let Optional { channel } = arguments(&Value::Null).expect("no arguments are an empty object");
        assert!(channel.is_none());

        let missing = arguments::<Note>(&json!({})).unwrap_err();
        assert!(matches!(&missing, ToolError::InvalidParameter(m) if m.contains("`text`")), "serde names what is missing: {missing}");
        let wrong = arguments::<Note>(&json!({ "text": 5 })).unwrap_err();
        assert!(matches!(&wrong, ToolError::InvalidParameter(m) if m.contains("invalid type")), "{wrong}");
        let id: caelumora_model::note::NoteId = id_argument(&json!({ "id": 3 })).unwrap();
        assert_eq!(id.0, 3);
        assert_eq!(text_argument(&json!({ "text": "  tea " })).unwrap(), "tea", "trimmed");
        assert!(matches!(text_argument(&json!({ "text": " " })), Err(ToolError::InvalidParameter(_))), "empty is no text");
    }

    /// `test_tool` does not classify its result; `hr_tool` classifies it as
    /// HR's; `configured_tool` does not classify, but is configured as HR's.
    #[derive(Debug)]
    struct MockToolProvider {
        requires_approval: bool,
    }

    fn hr() -> Classification {
        Classification::new(caelumora_model::confidentiality::Confidentiality::Internal, [caelumora_model::confidentiality::Compartment::new("hr")]).unwrap()
    }

    fn tool(name: &str, requires_approval: bool, classification: Option<Classification>) -> Tool {
        Tool {
            name: name.to_string(),
            description: "desc".to_string(),
            parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval,
            visibility: Default::default(),
            classification,
            presentation: Default::default(),
        }
    }

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn description(&self) -> String {
            "mock tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            vec![
                tool("test_tool", self.requires_approval, None),
                tool("hr_tool", self.requires_approval, None),
                tool("configured_tool", self.requires_approval, Some(hr())),
                tool("hanging_tool", self.requires_approval, None),
            ]
        }
        async fn execute(&self, name: &str, _params: &Value, _answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            match name {
                "hr_tool" => Ok(ToolOutput::classified(json!({"salary": 1}), hr())),
                "configured_tool" => Ok(json!({"salary": 2}).into()),
                "hanging_tool" => std::future::pending().await,
                _ => Ok(json!({"result": "ok"}).into()),
            }
        }
    }

    #[derive(Debug)]
    struct DenyAllSecurityManager;

    #[async_trait]
    impl SecurityManager for DenyAllSecurityManager {
        async fn check(&self, _context: &caelumora_model::security::SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), visibility: Default::default(), presentation: Default::default() }
    }

    fn parse(result: &str) -> Value {
        serde_json::from_str(result).expect("an unsuccessful result is a JSON object")
    }

    #[test]
    fn each_error_maps_to_its_own_status_with_a_next_step() {
        let available = vec!["calculator".to_string(), "echo".to_string()];
        let cases = [
            (ToolError::UnknownTool("shell".to_string()), ToolResultStatus::UnknownTool, "Available tools: calculator, echo"),
            (ToolError::InvalidParameter("'x' is missing".to_string()), ToolResultStatus::InvalidArguments, "call the tool again"),
            (ToolError::ExecutionFailed("disk full".to_string()), ToolResultStatus::Failed, "disk full"),
            (ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string()), ToolResultStatus::Forbidden, "not permitted"),
        ];
        for (error, expected, fragment) in cases {
            let (status, result) = error.to_result(&available);
            assert_eq!(status, expected);
            let json = parse(&result);
            assert_eq!(json["status"], serde_json::to_value(expected).unwrap());
            let message = json["message"].as_str().unwrap();
            assert!(message.contains(fragment), "{:?}: {}", expected, message);
        }
    }

    #[test]
    fn a_policy_refusal_does_not_disclose_the_policy() {
        let (_, result) = ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string()).to_result(&[]);
        assert!(!result.contains("rule 7"), "{}", result);
        assert_eq!(result, forbidden_result());
    }

    #[test]
    fn denial_and_timeout_are_distinguishable() {
        assert_eq!(parse(&denied_result())["status"], "denied");
        assert_eq!(parse(&timed_out_result())["status"], "timed_out");
        assert!(denied_result().contains("Do not repeat it"));
        assert!(timed_out_result().contains("ask the user whether to try again"));
    }

    fn executor(security: Arc<dyn SecurityManager>) -> GuardedToolExecutor {
        guarded(security, true, Places::default())
    }

    /// The rules of the places an agent works in (issue 13).
    #[derive(Default)]
    struct Places {
        system: Vec<ApprovalRule>,
        session: Vec<ApprovalRule>,
        channel: Vec<ApprovalRule>,
    }

    /// The mock tools - each requiring approval if `requires_approval` -
    /// guarded by `security` and the rules of `places`; the controllers of
    /// the session and the channel are returned, for their rules to be
    /// changed.
    fn guarded_in(security: Arc<dyn SecurityManager>, requires_approval: bool, places: Places) -> (GuardedToolExecutor, Arc<SessionController>, Arc<ChannelController>) {
        guarding(security, vec![("tools", Arc::new(MockToolProvider { requires_approval }))], places)
    }

    /// The tools of `providers`, by their component ids, guarded by
    /// `security` and the rules of `places`.
    fn guarding(security: Arc<dyn SecurityManager>, providers: Vec<(&str, Arc<dyn ToolProvider>)>, places: Places) -> (GuardedToolExecutor, Arc<SessionController>, Arc<ChannelController>) {
        let providers = providers.into_iter().map(|(id, facet)| ComponentImpl { id: id.to_string(), fail_policy: Default::default(), facet }).collect();
        let tools = DefaultToolExecutor::new(providers).unwrap();
        let store = Arc::new(caelumora_model::store::memory::MemoryStore::new());
        let session = crate::runtime::bare_session(caelumora_model::session::SessionId::new());
        let mut channel = caelumora_model::channel::Channel::build(session.id, ActorId::None, caelumora_model::value::name::Name::new("main").unwrap(), Default::default(), Classification::CHANNEL_DEFAULT, None, Vec::new());
        channel.approval_rules = places.channel;
        let session = Arc::new(SessionController::new(caelumora_model::session::Session { approval_rules: places.session, ..session }, Arc::new(()), Arc::clone(&store) as Arc<dyn caelumora_model::store::EntityStore>));
        let channel = Arc::new(ChannelController::new(channel, Arc::new(()), store));
        let executor = GuardedToolExecutor::new(security, ComponentImpl { id: "executor".to_string(), fail_policy: Default::default(), facet: Arc::new(tools) }, places.system.into(), Arc::clone(&session), Arc::clone(&channel), Default::default());
        (executor, session, channel)
    }

    fn guarded(security: Arc<dyn SecurityManager>, requires_approval: bool, places: Places) -> GuardedToolExecutor {
        guarded_in(security, requires_approval, places).0
    }

    fn rule(tool: &str, decision: RuleDecision) -> ApprovalRule {
        ApprovalRule::new(caelumora_model::approval::ToolMatcher { tool: tool.to_string(), arguments: Default::default() }, decision, Default::default(), None).unwrap()
    }

    #[tokio::test]
    async fn a_rule_of_a_place_denies_a_call_or_asks_for_it_explicitly() {
        let security: Arc<dyn SecurityManager> = Arc::new(DummySecurityManager::new());
        let admitted = |executor: GuardedToolExecutor| async move {
            let mut agent_channel_state = agent_turn(&executor);
            let turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
            executor.admit(&call("test_tool"), &turn).await
        };

        let denied = admitted(guarded(Arc::clone(&security), false, Places { system: vec![rule("test_tool", RuleDecision::Deny)], ..Default::default() })).await;
        assert!(matches!(denied, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden && r.content == forbidden_result()), "the system's, as the policy refuses: {denied:?}");
        let asked = admitted(guarded(Arc::clone(&security), false, Places { channel: vec![rule("test_tool", RuleDecision::Ask)], ..Default::default() })).await;
        assert!(matches!(asked, Admission::Ask { explicit: true, .. }), "the channel's, of a tool that would not ask: {asked:?}");
        let both = admitted(guarded(Arc::clone(&security), false, Places { session: vec![rule("*", RuleDecision::Ask)], channel: vec![rule("test_tool", RuleDecision::Deny)], ..Default::default() })).await;
        assert!(matches!(both, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden), "the strongest of all places: {both:?}");
        let other = admitted(guarded(Arc::clone(&security), false, Places { session: vec![rule("hr_tool", RuleDecision::Deny)], ..Default::default() })).await;
        assert!(matches!(other, Admission::Run), "a rule about another tool: {other:?}");
        let default = admitted(guarded(security, true, Places::default())).await;
        assert!(matches!(default, Admission::Ask { explicit: false, .. }), "the tool's own default is no rule: {default:?}");
    }

    #[tokio::test]
    async fn a_rule_that_denies_set_while_a_call_waits_refuses_it_when_run() {
        let (executor, session, _channel) = guarded_in(Arc::new(DummySecurityManager::new()), true, Places::default());
        let mut agent_channel_state = agent_turn(&executor);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        assert!(matches!(executor.admit(&call("test_tool"), &turn).await, Admission::Ask { .. }));

        // As if set in another instance, and announced.
        let denying = caelumora_model::session::SessionUpdateEvent::ApprovalRulesChanged(vec![rule("test_tool", RuleDecision::Deny)]);
        session.process_global_event(&caelumora_model::event::GlobalEvent::SessionUpdated(session.id(), denying));
        let result = executor.execute(&call("test_tool"), None, &mut turn, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "as it stands when run");
        assert!(result.started_at.is_none(), "it never ran");
    }

    #[tokio::test]
    async fn the_policy_is_checked_before_approval_and_again_on_execution() {
        let executor = executor(Arc::new(DenyAllSecurityManager));

        let mut agent_channel_state = AgentChannelState::default();
        let agent_id = AgentId::new();
        agent_channel_state.turn = Some(caelumora_model::turn::TurnState { work: caelumora_model::work::Work { security_context: SecurityContext { caller_id: ActorId::Agent(agent_id), ..SecurityContext::default() }, ..Default::default() }, tools: executor.executor.tool_sets().into_iter().flat_map(|set| set.tools).collect(), ..Default::default() });
        let mut context = Turn::of(agent_id, "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");

        let admitted = executor.admit(&call("test_tool"), &context).await;
        assert!(matches!(admitted, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden), "refused before anyone is asked: {admitted:?}");
        let result = executor.execute(&call("test_tool"), None, &mut context, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden);
        assert_eq!(result.content, forbidden_result());
        assert!(result.started_at.is_none() && result.finished_at.is_none(), "a refused call never ran");
    }

    #[tokio::test]
    async fn a_call_is_asked_for_run_or_answered_at_once_as_it_can_succeed() {
        let asking = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = agent_turn(&asking);
        let turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        assert!(matches!(asking.admit(&call("test_tool"), &turn).await, Admission::Ask { description, explicit: false, .. } if description == "desc"), "its tool requires approval");
        match asking.admit(&call("does_not_exist"), &turn).await {
            Admission::Answered(result) => {
                assert_eq!(result.status, ToolResultStatus::UnknownTool);
                assert!(result.content.contains("Available tools: test_tool"), "the tools the turn began with: {}", result.content);
            }
            other => panic!("an unknown tool is answered at once, got {other:?}"),
        }

        let refusing = executor(Arc::new(DenyAllSecurityManager));
        let mut agent_channel_state = agent_turn(&refusing);
        let turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        assert!(matches!(refusing.admit(&call("test_tool"), &turn).await, Admission::Answered(r) if r.status == ToolResultStatus::Forbidden), "first the policy");

        let running = guarded(Arc::new(DummySecurityManager::new()), false, Places::default());
        let mut agent_channel_state = agent_turn(&running);
        let turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        let admitted = running.admit(&call("test_tool"), &turn).await;
        assert!(matches!(admitted, Admission::Run), "no approval needed");
        let state = admitted.state(call("test_tool"));
        assert!(matches!(state.status, caelumora_model::tool::ToolCallStatus::Ready { answers: None }), "to be run");
    }

    /// A tool whose arguments go to a remote side cleared for
    /// `destination` (issue 180).
    #[derive(Debug)]
    struct Outward {
        destination: Classification,
    }

    #[async_trait]
    impl ToolProvider for Outward {
        fn description(&self) -> String {
            "sends out".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            vec![tool("send_out", false, None)]
        }
        fn destination(&self) -> Option<Classification> {
            Some(self.destination.clone())
        }
        async fn execute(&self, _: &str, _: &Value, _: Option<&Answers>, _: &mut Turn) -> ToolResult<ToolOutput> {
            Ok(json!({ "sent": true }).into())
        }
    }

    #[tokio::test]
    async fn a_set_whose_remote_side_the_channel_may_not_flow_to_is_withheld_and_its_tools_not_called() {
        use caelumora_model::confidentiality::Confidentiality;
        let security: Arc<dyn SecurityManager> = Arc::new(caelumora_model::security::OwnershipSecurityManager::new());
        let outward: Arc<dyn ToolProvider> = Arc::new(Outward { destination: Classification::general(Confidentiality::Internal) });
        let local: Arc<dyn ToolProvider> = Arc::new(MockToolProvider { requires_approval: false });
        let (executor, _, _) = guarding(security, vec![("outward", outward), ("local", local)], Places::default());
        let agent = SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() };

        let internal = executor.tool_sets(&agent, &Classification::general(Confidentiality::Internal)).await;
        assert!(internal.iter().all(|set| set.unavailable.is_none() && !set.tools.is_empty()), "offered where the channel flows to it: {internal:?}");
        assert_eq!(internal[0].destination, Some(Classification::general(Confidentiality::Internal)), "and says where");

        let confidential = executor.tool_sets(&agent, &Classification::general(Confidentiality::Confidential)).await;
        let (outward, local) = (&confidential[0], &confidential[1]);
        assert!(outward.tools.is_empty() && outward.unavailable.as_deref().is_some_and(|why| why.contains("not cleared for this channel")), "withheld above it: {outward:?}");
        assert!(local.unavailable.is_none() && !local.tools.is_empty(), "a set that sends nothing out is offered");

        let mut agent_channel_state = AgentChannelState {
            turn: Some(caelumora_model::turn::TurnState {
                work: caelumora_model::work::Work { security_context: agent, classification: Classification::general(Confidentiality::Confidential), ..Default::default() },
                tools: confidential.into_iter().flat_map(|set| set.tools).collect(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        let admitted = executor.admit(&call("send_out"), &turn).await;
        assert!(matches!(&admitted, Admission::Answered(r) if r.status == ToolResultStatus::UnknownTool), "not called by its name: {admitted:?}");
        let result = executor.execute(&call("send_out"), None, &mut turn, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::UnknownTool, "nor run, approved or not");
        assert!(result.started_at.is_none());
    }

    #[test]
    fn approval_is_configured_for_all_none_or_all_but_what_only_reads() {
        let configured = |value: Value| serde_json::from_value::<ApprovalMode>(value);
        assert_eq!(configured(json!(true)).unwrap(), ApprovalMode::Always);
        assert_eq!(configured(json!(false)).unwrap(), ApprovalMode::Never);
        assert_eq!(configured(json!("unless_read_only")).unwrap(), ApprovalMode::UnlessReadOnly);
        assert!(configured(json!("hints")).is_err());
        for (approval, asks) in [(ApprovalMode::Always, [true, true]), (ApprovalMode::Never, [false, false]), (ApprovalMode::UnlessReadOnly, [false, true])] {
            assert_eq!([true, false].map(|read_only| approval.required(read_only)), asks, "{approval:?}: reading, writing");
        }
    }

    #[tokio::test]
    async fn only_an_agent_calls_tools() {
        let executor = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::turn::TurnState { work: caelumora_model::work::Work { security_context: SecurityContext { caller_id: ActorId::User(caelumora_model::user::UserId::new()), ..SecurityContext::default() }, ..Default::default() }, tools: executor.executor.tool_sets().into_iter().flat_map(|set| set.tools).collect(), ..Default::default() });
        let mut context = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");

        let result = executor.execute(&call("test_tool"), None, &mut context, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "whatever the policy says");
    }

    /// A turn of an agent in a general internal channel, with the tools of
    /// `executor` as it began with them.
    fn agent_turn(executor: &GuardedToolExecutor) -> AgentChannelState {
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::turn::TurnState {
            work: caelumora_model::work::Work { security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() }, ..Default::default() },
            tools: executor.executor.tool_sets().into_iter().flat_map(|set| set.tools).collect(),
            ..Default::default()
        });
        agent_channel_state
    }

    fn with_hr() -> Arc<dyn SecurityManager> {
        let compartments = caelumora_model::confidentiality::Compartments::new([(caelumora_model::confidentiality::Compartment::new("hr"), vec![])]).unwrap();
        Arc::new(caelumora_model::security::OwnershipSecurityManager::with_rules(caelumora_model::confidentiality::ConfidentialityRules::new(compartments)))
    }

    #[tokio::test]
    async fn a_call_runs_is_timed_and_classified_as_the_channel_unless_it_says_otherwise() {
        let executor = executor(with_hr());
        let mut agent_channel_state = agent_turn(&executor);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");

        let result = executor.execute(&call("test_tool"), None, &mut turn, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert_eq!(result.content, "{\"result\":\"ok\"}");
        assert_eq!(result.classification, Classification::CHANNEL_DEFAULT, "the channel's");
        let (started_at, finished_at) = (result.started_at.expect("a call that ran has its start"), result.finished_at.expect("and its end"));
        assert!(finished_at >= started_at);
        assert!(serde_json::to_string(&result).unwrap().contains("\"started_at\""), "and it is stored with the history");

        let unknown = executor.execute(&call("does_not_exist"), None, &mut turn, &CancellationToken::new()).await;
        assert_eq!(unknown.status, ToolResultStatus::UnknownTool);
        assert!(unknown.content.contains("Available tools: test_tool"), "{}", unknown.content);
        assert_eq!((unknown.id.as_deref(), unknown.tool_name.as_str()), (Some("call_1"), "does_not_exist"), "the result is the call's");
        assert!(unknown.started_at.is_none(), "it never ran");
    }

    #[tokio::test]
    async fn a_call_still_running_when_its_turn_ends_is_interrupted_and_counts_as_run() {
        let executor = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = agent_turn(&executor);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        let cancelled = CancellationToken::new();
        let ending = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            ending.cancel();
        });

        let result = executor.execute(&call("hanging_tool"), None, &mut turn, &cancelled).await;
        assert_eq!(result.status, ToolResultStatus::Interrupted, "not waited for once the turn ends");
        assert_eq!(result.content, interrupted_result());
        let (started_at, finished_at) = (result.started_at.expect("it ran"), result.finished_at.expect("until it was interrupted"));
        assert!(finished_at >= started_at);

        let done = executor.execute(&call("test_tool"), None, &mut turn, &cancelled).await;
        assert_eq!(done.status, ToolResultStatus::Ok, "a tool that is done has its result");
    }

    #[tokio::test]
    async fn a_result_that_may_not_flow_into_the_channel_never_reaches_the_model() {
        let executor = executor(with_hr());
        let mut agent_channel_state = agent_turn(&executor);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");

        for name in ["hr_tool", "configured_tool"] {
            let result = executor.execute(&call(name), None, &mut turn, &CancellationToken::new()).await;
            assert_eq!(result.status, ToolResultStatus::Withheld, "{name}: HR's, and the channel is general");
            assert!(!result.content.contains("salary"), "{name}: {}", result.content);
            assert!(result.content.contains("internal {hr}"), "the model is told why: {}", result.content);
            assert_eq!(result.classification, Classification::CHANNEL_DEFAULT, "what the model is told is the channel's");
            assert!(result.started_at.is_some(), "it ran");
        }
    }

    #[tokio::test]
    async fn a_result_classified_by_the_tool_is_kept_where_the_channel_admits_it() {
        let executor = executor(with_hr());
        let mut agent_channel_state = agent_turn(&executor);
        if let Some(turn) = agent_channel_state.turn.as_mut() {
            turn.work.classification = hr();
        }
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        let result = executor.execute(&call("hr_tool"), None, &mut turn, &CancellationToken::new()).await;
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert_eq!(result.classification, hr(), "as the tool classified it");
    }

    #[test]
    fn a_result_is_written_without_its_status_when_ok_and_read_back() {
        let result: ToolCallResult = serde_json::from_value(json!({"id": "1", "tool_name": "echo", "content": "hi", "classification": {"level": "internal"}})).unwrap();
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert!(!serde_json::to_string(&result).unwrap().contains("status"), "Ok is not written");

        let denied = ToolCallResult { status: ToolResultStatus::Denied, ..result };
        assert!(serde_json::to_string(&denied).unwrap().contains("\"status\":\"denied\""));
    }

    #[test]
    fn a_result_the_channel_does_not_see_is_marked_for_the_model() {
        let value = json!({"secret": 42});
        let public = successful_result(value.clone(), ToolVisibility::SHOWN);
        assert_eq!(parse(&public), value, "passed on unchanged");

        let hidden = parse(&successful_result(value.clone(), ToolVisibility::CALL_ONLY));
        assert_eq!(hidden["result"], value);
        assert!(hidden["note"].as_str().unwrap().contains("Do not quote or reveal it"));
    }
}
