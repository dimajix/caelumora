//! Tools (issue 52): the facet `ToolProvider`, the `ToolExecutor` that runs an agent's tool calls
//! with the tools of its providers and with the checks the runtime keeps to itself (issues 127,
//! 280), what tools share - a text given a page at a time (`page_of`, issue 282) -, and the
//! built-in tool components, one file per component kind. A component that offers tools along with
//! another facet - a prompt part, say - stays in `components`.

pub mod ask_user_tool;
pub mod attachment_tool;
pub mod calculator_tool;
pub mod echo_tool;
mod page;
mod providers;
pub mod sleep_tool;
pub mod stop_tool;

pub use page::{page_of, TextPage};

use crate::prompt::NAMESPACE;
use crate::situation::Situation;
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId, RuleDecision, ToolMatcher};
use caelumora_model::confidentiality::{Classification, Compartment, Confidentiality};
use caelumora_model::content::ContentRef;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::question::{Answers, Questions};
use caelumora_model::security::{ActionType, Enforcement, Policies};
use caelumora_model::tool::{
    Tool, ToolCall, ToolCallResult, ToolCallState, ToolParameters, ToolResultStatus, ToolSet,
    ToolVisibility,
};
use caelumora_model::turn::GivenUpReason;
use caelumora_runtime::attachment::AttachmentError;
use caelumora_runtime::backend::StorageGone;
use caelumora_runtime::component::{ComponentImpl, ComponentRegistry, ComponentResult};
use caelumora_runtime::content::ContentError;
use caelumora_runtime::controller::{ChannelController, SessionController};
use caelumora_runtime::runtime::ActorRuntime;
use caelumora_runtime::terminal::TerminalError;
use caelumora_runtime::view::{ViewError, WorkspaceView};
use caelumora_runtime::workspace::WorkspaceError;
use chrono::Utc;
use providers::ToolProviders;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use std::future::Future;
use std::iter;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

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

/// What a model is told when a tool of its agent could not follow an edge
/// of the views: what cannot be reached is the model's to correct; a
/// failure is the tool's.
impl From<ViewError> for ToolError {
    fn from(error: ViewError) -> Self {
        match error {
            ViewError::Forbidden { .. } => ToolError::PermissionDenied(error.to_string()),
            ViewError::Failed(_) => ToolError::ExecutionFailed(error.to_string()),
            ViewError::Attachment(error) => error.into(),
            _ => ToolError::InvalidParameter(error.to_string()),
        }
    }
}

/// What reaching an attachment failed with (issue 282), as the model is told it: a storage that
/// failed, is no longer configured, or holds other bytes than were kept is the tool's; an
/// attachment that is not there, or what does not fit, the model's to correct.
impl From<AttachmentError> for ToolError {
    fn from(error: AttachmentError) -> Self {
        match error {
            AttachmentError::Content(e) => e.into(),
            AttachmentError::StorageGone(e) => e.into(),
            AttachmentError::Altered(_) => ToolError::ExecutionFailed(error.to_string()),
            AttachmentError::NotConfigured
            | AttachmentError::ClassificationRefused
            | AttachmentError::StorageNotCleared(_)
            | AttachmentError::NotAParticipant(_)
            | AttachmentError::NotFound(_)
            | AttachmentError::Invalid(_) => ToolError::InvalidParameter(error.to_string()),
        }
    }
}

/// That the workspace could not be used as a call needs it, as the model is told it: the tool's
/// failure - a storage no longer configured without its name -, and what its place reported as a
/// terminal's error is told.
impl From<WorkspaceError> for ToolError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::StorageGone(e) => e.into(),
            WorkspaceError::Terminal(e) => e.into(),
            error => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

/// A storage that what is kept names, no longer configured (issue 466), as the model is told it:
/// the tool's failure, without the name, which is the configuration's - as a user is told it.
impl From<StorageGone> for ToolError {
    fn from(_: StorageGone) -> Self {
        ToolError::ExecutionFailed(
            "the storage of what was asked for is no longer configured".to_string(),
        )
    }
}

/// What a storage of content reported (issue 466), as the model is told it: content too large,
/// or other than it was said to be, is the model's to correct; a storage that failed, or a key it
/// does not know, is not.
impl From<ContentError> for ToolError {
    fn from(error: ContentError) -> Self {
        match error {
            ContentError::TooLarge { .. } | ContentError::Changed => {
                ToolError::InvalidParameter(error.to_string())
            }
            ContentError::Unavailable(_)
            | ContentError::InvalidKey(_)
            | ContentError::NotFound(_) => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

/// What a terminal reports, as the model is told it: what names something
/// that is not there, or is not what the call needs, is the model's to
/// correct; that the storage failed is not.
impl From<TerminalError> for ToolError {
    fn from(error: TerminalError) -> Self {
        match error {
            TerminalError::Unavailable(_) => ToolError::ExecutionFailed(error.to_string()),
            TerminalError::InvalidPath(_)
            | TerminalError::NotFound(_)
            | TerminalError::Exists(_)
            | TerminalError::IsADirectory(_)
            | TerminalError::NotADirectory(_)
            | TerminalError::NotEmpty(_)
            | TerminalError::TooLarge { .. }
            | TerminalError::UnknownCommand(_) => ToolError::InvalidParameter(error.to_string()),
        }
    }
}

/// What a tool gives back (issue 133): its value and, if the tool
/// classifies it - by its input, by what it read, by the source it drew
/// on - how sensitive it is. `None` leaves it to the tool's configuration
/// or the channel (`ToolExecutor`). And the attachments of its channel it
/// shows its model again (issue 282), which the model is given with the
/// result as it is given an attachment (`ToolCallResult::shown`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub classification: Option<Classification>,
    pub shown: Vec<ContentRef>,
}

impl ToolOutput {
    /// `value`, classified `classification` by the tool.
    pub fn classified(value: Value, classification: Classification) -> Self {
        Self { value, classification: Some(classification), shown: Vec::new() }
    }
}

/// A value the tool does not classify.
impl From<Value> for ToolOutput {
    fn from(value: Value) -> Self {
        Self { value, classification: None, shown: Vec::new() }
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
    struct Id<T> {
        id: T,
    }
    let Id { id } = arguments(params)?;
    Ok(id)
}

/// The parameters `level` and `compartments` of a tool that reclassifies
/// something (issue 133), as a classification. One that cannot be - a
/// public one with compartments - is the model's to correct.
pub fn classification_argument(params: &Value) -> ToolResult<Classification> {
    #[derive(Deserialize)]
    struct Asked {
        level: Confidentiality,
        #[serde(default)]
        compartments: Vec<String>,
    }
    let Asked { level, compartments } = arguments(params)?;
    Classification::new(level, compartments.into_iter().map(Compartment::new))
        .map_err(|e| ToolError::InvalidParameter(e.to_string()))
}

/// The text `key` of `prompts` about the tools the component kind `kind`
/// offers (issue 57), among the texts of `namespace` - the agents' or a
/// plugin's -, rendered with `values`: `description` that of its tool set,
/// `<tool>` that of a tool, `<tool>.<path>` that of a parameter
/// (`ToolParameters::describe`). The texts of tools are rendered once, when
/// their component is made.
///
/// # Errors
/// `PromptError` if the catalogue has no such text, or it cannot be
/// rendered with `values`.
pub fn tool_text(
    prompts: &PromptTexts,
    namespace: &str,
    kind: &str,
    key: &str,
    values: impl Serialize,
) -> Result<String, PromptError> {
    prompts.render(&format!("{namespace}.tools.{kind}.{key}"), values)
}

/// `tool`, one of the component kind `kind`, with its description and
/// those of its parameters not described yet from the texts of `namespace`
/// in `prompts`, rendered with `values` (`tool_text`).
///
/// # Errors
/// As `tool_text`, for the first text that cannot be had.
pub fn described(
    prompts: &PromptTexts,
    namespace: &str,
    kind: &str,
    mut tool: Tool,
    values: impl Serialize,
) -> Result<Tool, PromptError> {
    tool.description = tool_text(prompts, namespace, kind, &tool.name, &values)?;
    tool.parameters
        .describe(&tool.name, &mut |path| tool_text(prompts, namespace, kind, path, &values))?;
    Ok(tool)
}

/// The parameters of a tool that reclassifies the entry numbered `id`:
/// `id`, which the tool's own text describes (`described`), and `level`
/// and `compartments`, described by the texts every such tool shares.
///
/// # Errors
/// As `tool_text`.
pub fn reclassify_parameters(prompts: &PromptTexts) -> Result<ToolParameters, PromptError> {
    let shared = |key| tool_text(prompts, NAMESPACE, "reclassify", key, ());
    Ok(ToolParameters::object(
        [
            ("id", ToolParameters::number()),
            ("level", ToolParameters::string().described(shared("level")?)),
            (
                "compartments",
                ToolParameters::array(
                    ToolParameters::string().described(shared("compartments.item")?),
                )
                .described(shared("compartments")?),
            ),
        ],
        ["id", "level"],
    ))
}

/// The parameter `text` of a tool that notes or remembers something,
/// trimmed; an empty one is no text.
pub fn text_argument(params: &Value) -> ToolResult<String> {
    #[derive(Deserialize)]
    struct Text {
        text: String,
    }
    let Text { text } = arguments(params)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(ToolError::InvalidParameter("'text' must not be empty".to_string()));
    }
    Ok(text.to_string())
}

/// The text `key` of `prompts` for the result of a tool call (issue 57),
/// rendered with `values`.
pub(crate) fn result_text(
    prompts: &PromptTexts,
    key: &str,
    values: impl Serialize,
) -> Result<String, PromptError> {
    prompts.render(&format!("{NAMESPACE}.results.{key}"), values)
}

/// What the model is given as the result of a tool call that produced none
/// (issue 15): a small JSON object with the `status` and a `message` that
/// says what happened and what to do next, e.g.
/// `{"status":"denied","message":"The user declined ..."}`. Successful
/// results are passed on unchanged. A `message` that cannot be had is
/// logged, and the model is given the status alone: what happened, if not
/// what to do next - the call is answered all the same.
fn unsuccessful_result(status: ToolResultStatus, message: Result<String, PromptError>) -> String {
    match message {
        Ok(message) => serde_json::json!({ "status": status, "message": message }).to_string(),
        Err(e) => {
            error!("The model is told only the status '{status:?}' of a tool call: {e}");
            serde_json::json!({ "status": status }).to_string()
        }
    }
}

/// What the model is given as the result of a call that succeeded: the
/// tool's `value` - marked as the agent's alone, by the text of `prompts`,
/// if the channel does not see it (issue 66), i.e. if the call's result is
/// its owner's (`Visibility::Owner`). The answer text is always published,
/// so a result the channel does not see is worthless redacted if the model
/// quotes it. A value that is text - a skill loaded, the text of a remote
/// tool - is given as the text it is (issue 22), not as a JSON string; any
/// other as JSON.
///
/// # Errors
/// `PromptError` if the mark is needed and cannot be had: the result is
/// not to be given without it.
pub fn successful_result(
    prompts: &PromptTexts,
    value: Value,
    visibility: ToolVisibility,
) -> Result<String, PromptError> {
    if visibility.result.is_channel() {
        return Ok(match value {
            Value::String(text) => text,
            value => value.to_string(),
        });
    }
    let owners = result_text(prompts, "owners", ())?;
    Ok(match value {
        Value::String(text) => format!("{text}\n\n[{owners}]"),
        value => serde_json::json!({ "result": value, "note": owners }).to_string(),
    })
}

/// The result for a call a user declined.
pub(crate) fn denied_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::Denied, result_text(prompts, "denied", ()))
}

/// The result for a call nobody approved in time.
pub(crate) fn timed_out_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::TimedOut, result_text(prompts, "timed_out", ()))
}

/// The result for a call whose questions a person declined to answer
/// (issue 165), by saying so or by writing a message instead.
pub(crate) fn declined_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::Declined, result_text(prompts, "declined", ()))
}

/// The result for a call whose questions nobody asked is left to answer
/// (issue 482): those asked left the channel, and it was not executed.
pub(crate) fn unanswered_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::NotExecuted, result_text(prompts, "unanswered", ()))
}

/// The result for a call that was approved but has no result when its
/// agent is loaded again (issue 75): the server or the session stopped
/// before the call ran (issue 377), or before its result was stored. The
/// runtime cannot tell which, so it never runs the call again - the model
/// decides.
pub(crate) fn not_executed_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::NotExecuted, result_text(prompts, "not_executed", ()))
}

/// The result for a call that was running when its turn was cancelled or
/// given up (issue 161): it was interrupted, and what it did before stays
/// done. The runtime cannot tell how far it got - the model checks before
/// it calls it again.
pub(crate) fn interrupted_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::Interrupted, result_text(prompts, "interrupted", ()))
}

/// Why a running call is broken off (issues 161, 276), which decides what
/// the model is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interruption {
    /// Its turn was cancelled or given up: no further call of the step runs.
    TurnEnded,
    /// Its channel stopped (issue 377): the turn is kept, to be resumed;
    /// the calls of the step still to run are not run then (issue 75).
    Stopped,
    /// A user, called so, interrupted this call alone: the turn goes on.
    ByUser(String),
}

impl Interruption {
    /// The result of a call interrupted so, `after` it ran so long, worded
    /// by `prompts`.
    fn result(&self, prompts: &PromptTexts, after: Duration) -> String {
        let seconds = format!("{:.1}", after.as_secs_f64());
        match self {
            Self::TurnEnded => interrupted_result(prompts),
            Self::Stopped => unsuccessful_result(
                ToolResultStatus::Interrupted,
                result_text(prompts, "interrupted_stopped", context! { seconds }),
            ),
            Self::ByUser(by) => unsuccessful_result(
                ToolResultStatus::Interrupted,
                result_text(prompts, "interrupted_by_user", context! { by, seconds }),
            ),
        }
    }
}

/// The result for a call that a user, `by`, interrupted before it ran
/// (issue 276): it was not run, and the turn goes on.
pub(crate) fn skipped_result(prompts: &PromptTexts, by: &str) -> String {
    unsuccessful_result(
        ToolResultStatus::NotExecuted,
        result_text(prompts, "skipped", context! { by }),
    )
}

/// The result for a call that had no result when its turn ended (issue
/// 148): it never ran, because of what the text `cause` of `prompts` says -
/// "the turn ended before it ran, because it was cancelled" (`TurnEnd`).
pub(crate) fn abandoned_result(prompts: &PromptTexts, cause: &TurnEnd) -> String {
    let message = prompts
        .render(&cause.key(), ())
        .and_then(|cause| result_text(prompts, "abandoned", context! { cause }));
    unsuccessful_result(ToolResultStatus::NotExecuted, message)
}

/// Why a turn ended with calls that never ran (issue 148), as the model is
/// told it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TurnEnd<'a> {
    /// It was taken over as it stood, cancelled or not.
    Committed { cancelled: bool },
    /// It was given up for its reason.
    GivenUp(&'a GivenUpReason),
}

impl TurnEnd<'_> {
    /// The key of the text that says why: the core's for a turn given up
    /// (`core.given_up_cause`), the agents' otherwise.
    fn key(&self) -> String {
        match self {
            TurnEnd::Committed { cancelled: true } => {
                format!("{NAMESPACE}.results.ended_because.cancelled")
            }
            TurnEnd::Committed { cancelled: false } => {
                format!("{NAMESPACE}.results.ended_because.ended")
            }
            TurnEnd::GivenUp(reason) => reason.cause_key(),
        }
    }
}

/// The result for a call whose result is classified `classification`, which
/// may not flow into the channel (issue 133). The model is told the
/// classification, so that it can tell the user why, and not to look for
/// the result elsewhere.
pub(crate) fn withheld_result(prompts: &PromptTexts, classification: &Classification) -> String {
    unsuccessful_result(
        ToolResultStatus::Withheld,
        result_text(prompts, "withheld", context! { classification => classification.to_string() }),
    )
}

/// The result for a call the policy does not permit. Deliberately without the
/// reason: telling the model why describes the policy to something that may
/// try to route around it. The reason is in the log.
pub(crate) fn forbidden_result(prompts: &PromptTexts) -> String {
    unsuccessful_result(ToolResultStatus::Forbidden, result_text(prompts, "forbidden", ()))
}

impl ToolError {
    /// The status and the model-facing result, worded by `prompts`, for a
    /// call that failed with this error. `available` names the tools that do
    /// exist, offered when the model called one that does not. What went
    /// wrong is the error's own text, which is no text of the catalogue
    /// (issue 57).
    pub fn to_result(
        &self,
        prompts: &PromptTexts,
        available: &[String],
    ) -> (ToolResultStatus, String) {
        let (status, message) = match self {
            ToolError::PermissionDenied(_) => {
                return (ToolResultStatus::Forbidden, forbidden_result(prompts))
            }
            ToolError::UnknownTool(name) => (
                ToolResultStatus::UnknownTool,
                result_text(prompts, "unknown_tool", context! { name, available }),
            ),
            ToolError::InvalidParameter(reason) => (
                ToolResultStatus::InvalidArguments,
                result_text(prompts, "invalid_arguments", context! { reason }),
            ),
            ToolError::ExecutionFailed(reason) => {
                (ToolResultStatus::Failed, result_text(prompts, "failed", context! { reason }))
            }
        };
        (status, unsuccessful_result(status, message))
    }
}

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// What its tools are for, written for a model: the description of its
    /// tool set, which is named after its component (issue 194).
    fn description(&self) -> String;
    /// Every tool it has: those it may offer, by which a call finds its
    /// provider, and whose names no other provider of the agent may have.
    fn tools(&self) -> Vec<Tool>;
    /// The tools of `tools` it offers the model in `work` (issue 290), asked
    /// as a turn begins: those that may be called there. `Err` with why it
    /// offers none, in general terms - never the error, which may name hosts
    /// (issue 32). By default all of them. A call is checked anyway as it
    /// runs: what was offered may change while the turn goes on. Asked of a
    /// turn just begun, and - for what an agent would be offered (the
    /// agent's `tool_sets`) - of work that is no turn: what it decides by
    /// is the place and who acts, not the turn's state.
    async fn offered(&self, _situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        Ok(self.tools())
    }
    /// Where the arguments of its calls go (issue 180): the classification
    /// its remote side is cleared for. By default none - they stay in the
    /// system; a provider that sends them out says where.
    fn destination(&self) -> Option<Classification> {
        None
    }

    /// Executes a specific tool by name - with the `answers` to the
    /// questions it asked for the call (`questions`), if it asked any.
    async fn execute(
        &self,
        name: &str,
        params: &Value,
        answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput>;
    /// What the call of tool `name` with `params` asks a person before it
    /// runs (issue 165): the answers are handed to `execute`, and reach the
    /// model only as far as the tool puts them into its result. By default
    /// nothing; an error is the call's, as with `describe`.
    fn questions(&self, _name: &str, _params: &Value) -> ToolResult<Option<Questions>> {
        Ok(None)
    }
    /// What other calls are like the call of tool `name` with `params`
    /// (issue 13): the matcher a rule derived from it is offered with. By
    /// default as the tool's parameters tell (`proposed_matcher`): every
    /// argument exactly, one written in a language by its meaning; a tool
    /// that knows more of what makes two calls alike says better.
    fn matcher(&self, name: &str, params: &Value) -> ToolMatcher {
        proposed_matcher(&self.tools(), name, params)
    }
    /// Provides a textual description for a specific tool call
    fn describe(&self, name: &str, _params: &Value) -> ToolResult<String> {
        let tools = self.tools();
        if let Some(tool) = tools.iter().find(|t| t.name == name) {
            Ok(tool.description.to_string())
        } else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
}

/// The workspace the tools of a set on it may be offered on in `situation`
/// (issue 290): the one of the agent's session the work may use in its
/// channel (`Situation::workspace_view`). `Err` with why the set is unavailable,
/// in general terms: there is none, or it cannot be read now - which is
/// logged.
pub async fn usable_workspace(situation: &Situation<'_>) -> Result<WorkspaceView, String> {
    match situation.workspace_view().await {
        Ok(Some(workspace)) => Ok(workspace),
        Ok(None) => Err(WorkspaceError::NotUsableInChannel.to_string()),
        Err(error) => {
            warn!("Reading the workspace an agent may use in its channel failed: {error}");
            Err("its workspace cannot be read now".to_string())
        }
    }
}

/// What a rule derived from a call of the tool `name` with `params` is
/// offered with, as the parameters of the tool of that name among `tools`
/// tell (`ToolMatcher::proposed`, issue 247); the calls exactly like it if
/// none has that name.
pub fn proposed_matcher(tools: &[Tool], name: &str, params: &Value) -> ToolMatcher {
    let call = ToolCall {
        id: None,
        function_name: name.to_string(),
        arguments: params.clone(),
        visibility: ToolVisibility::SHOWN,
        presentation: Default::default(),
    };
    match tools.iter().find(|tool| tool.name == name) {
        Some(tool) => ToolMatcher::proposed(&call, &tool.parameters),
        None => ToolMatcher::exact(&call),
    }
}

/// How a tool call of the model is admitted, before anything runs
/// (`ToolExecutor::admit`, issue 155).
#[derive(Debug)]
pub enum Admission {
    /// A user decides first; they are shown `description`. `explicit` if a
    /// rule of a place asked for it: a decision of their own (issue 13).
    /// `suggested` is what a rule derived from the call would match, if one
    /// can be kept (issue 400).
    /// Approved, it asks `questions`, if it has any (issue 165).
    Ask {
        description: String,
        explicit: bool,
        suggested: Option<ToolMatcher>,
        questions: Option<Questions>,
    },
    /// The users answer its questions first (issue 165), as put to them.
    Await(Questions),
    /// It runs as it is.
    Run,
    /// It cannot succeed and is answered at once, without asking anyone -
    /// boxed, since a result is much larger than the other variants.
    Answered(Box<ToolCallResult>),
}

impl Admission {
    /// The state `call` starts with, admitted so.
    pub fn state(self, call: ToolCall) -> ToolCallState {
        match self {
            Admission::Ask { description, explicit, suggested, questions } => {
                ToolCallState::pending(call, description, explicit, suggested, questions)
            }
            Admission::Await(questions) => ToolCallState::asking(call, questions),
            Admission::Run => ToolCallState::approved(call),
            Admission::Answered(result) => ToolCallState::answered(call, *result),
        }
    }
}

/// Which calls of a provider's tools require human approval, as configured
/// in its `requires_approval`: `true` (all of them, the default), `false`
/// (none), or `"unless_read_only"` - all but those of the tools that only
/// read (issue 246), as the provider tells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "Value")]
pub enum ApprovalMode {
    #[default]
    Always,
    Never,
    UnlessReadOnly,
}

impl ApprovalMode {
    /// Whether calls of a tool require approval, which only reads if
    /// `read_only`.
    pub fn required(self, read_only: bool) -> bool {
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
    turn.tools().any(|tool| tool.name == call.function_name)
}

/// Runs an agent's tool calls with the tools of its components' providers
/// (`ToolProviders`), and with what the runtime keeps to itself (issues
/// 127, 133, 280): only an agent calls tools, and only what the policy
/// permits (`CallTool`) - asked before a call is put up for approval
/// (`admit`, which also decides whether a user is asked) and again when
/// it is run. And the result the model is
/// given is made here, the one place: what the tool gave, or what went
/// wrong and what to do next (issue 15), classified - as the tool
/// classified it, else as the tool is configured, else as the channel -
/// and withheld if that may not flow into the channel, before the model
/// sees it. A tool only ever sees permitted calls, and never decides what
/// the model is told. It is no component: how a call runs is its tool's
/// business (issue 280), and what is checked is never the choice of whoever
/// may write a personality.
///
/// The rules of the places the agent works in (issue 13) - the system's,
/// its session's, its channel's - are applied here too, as they stand at
/// each call: a rule that denies refuses a call as the policy does, before
/// anyone is asked and again when it is run, and a rule that asks puts the
/// call to a person for a decision of their own, whatever its tool says.
#[derive(Debug)]
pub struct ToolExecutor {
    /// Who may call a tool (`CallTool`), and what may flow where (issue 304): a result into the
    /// agent's context, the channel's content to a tool set's remote side.
    policies: Policies,
    providers: ToolProviders,
    system_rules: Arc<[ApprovalRule]>,
    session: Arc<SessionController>,
    channel: Arc<ChannelController>,
    /// The agent's runtime, through which a call's questions are put to
    /// the users of its channel as it runs.
    runtime: ActorRuntime,
}

impl ToolExecutor {
    /// An executor for the tools of `providers`, in the order of the
    /// configuration, for the agent of `runtime`: guarded by the runtime's
    /// policies and by the rules of the system (`AppConfig::approvals`), of
    /// the agent's session and of its channel, whose users the questions of
    /// its calls are put to through `runtime` (issues 371, 374).
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two providers have a tool of the
    /// same name (`ToolProviders::new`).
    pub(crate) fn new(
        providers: Vec<ComponentImpl<dyn ToolProvider>>,
        runtime: ActorRuntime,
    ) -> ComponentResult<Self> {
        Ok(Self {
            policies: runtime.policies().clone(),
            providers: ToolProviders::new(providers)?,
            system_rules: Arc::clone(runtime.approval_rules()),
            session: Arc::clone(runtime.session_controller()),
            channel: Arc::clone(runtime.channel_controller()),
            runtime,
        })
    }

    /// What `call` asks before it runs, as its tool says, checked and put to
    /// the users of the channel (issue 165) - the agent's owner and whoever
    /// caused the call as the context of `turn` names them (issues 255,
    /// 341). Every error is the call's: it cannot be asked.
    fn questions(&self, call: &ToolCall, turn: &Turn<'_>) -> ToolResult<Option<Questions>> {
        let Some(questions) = self.providers.questions(call)? else { return Ok(None) };
        questions.check().map_err(|e| ToolError::InvalidParameter(e.to_string()))?;
        let addressed = self
            .runtime
            .address_questions(questions, turn.situation().security_context())
            .ok_or_else(|| ToolError::ExecutionFailed("the channel does not run".to_string()))?;
        addressed.map(Some).map_err(|e| ToolError::InvalidParameter(e.to_string()))
    }

    /// The rule of a place that decides `call` now, if one does, with what
    /// it decides of it: the strongest of the system's, the session's and the
    /// channel's (`ApprovalRule::decide`).
    fn place_rule(&self, call: &ToolCall) -> Option<(ApprovalRuleId, RuleDecision)> {
        let (session, channel) = (self.session.load(), self.channel.load());
        let rules =
            self.system_rules.iter().chain(&session.approval_rules).chain(&channel.approval_rules);
        ApprovalRule::decide(rules, call, session.id, channel.id, Utc::now())
            .map(|(rule, decision)| (rule.id, decision))
    }

    /// Runs `call` if it may be run now, with the `answers` to the questions
    /// its tool asked, and answers it either way. Once `interrupted` ends -
    /// the turn is cancelled or stopped (issue 161), or a user interrupts
    /// this call alone (issue 276) - the call is no longer waited for:
    /// whatever the executor and its tool are doing is dropped, and the call
    /// is answered as interrupted, saying why, with when it started, so that
    /// it counts as having run. Dropping a tool does not undo what it did
    /// (issue 20).
    pub async fn execute(
        &self,
        call: &ToolCall,
        answers: Option<&Answers>,
        turn: &mut Turn<'_>,
        interrupted: impl Future<Output = Interruption>,
    ) -> ToolCallResult {
        if let Err(e) = self.check(call, turn).await {
            return self.failed(call, e, turn);
        }
        let started_at = Utc::now();
        // A tool that is done when it is interrupted has its result.
        let ran = tokio::select! {
            biased;
            outcome = self.providers.execute(call, answers, turn) => Ok(outcome),
            interruption = interrupted => Err(interruption),
        };
        let finished_at = Utc::now();
        let outcome = match ran {
            Ok(outcome) => outcome,
            Err(interruption) => {
                let after = (finished_at - started_at).to_std().unwrap_or_default();
                info!(tool = %call.function_name, duration_ms = after.as_millis(), by_user = matches!(interruption, Interruption::ByUser(_)), "Tool interrupted");
                // A fixed text of the runtime's: as the turn (issue 168).
                let mut result = ToolCallResult::of(
                    call,
                    ToolResultStatus::Interrupted,
                    interruption.result(&turn.situation().texts(), after),
                    turn.situation().label(),
                );
                result.started_at = Some(started_at);
                result.finished_at = Some(finished_at);
                return result;
            }
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
        let available: Vec<String> = turn.tools().map(|t| t.name.clone()).collect();
        let (status, content) = error.to_result(&turn.situation().texts(), &available);
        ToolCallResult::of(call, status, content, turn.situation().classification())
    }

    /// The result for `call`, whose tool gave `output`: classified as the
    /// tool classified it, else as the tool is configured - as the turn
    /// began with its tools - else as the channel, and withheld - with what
    /// it showed (issue 282) - if that, or what it showed, may not flow into
    /// the channel (`ConfidentialityPolicy::may_publish`, in the agent's
    /// name: the result enters its context in the channel).
    async fn admitted(
        &self,
        call: &ToolCall,
        output: ToolOutput,
        turn: &Turn<'_>,
    ) -> ToolCallResult {
        let channel = turn.situation().classification();
        let configured = turn
            .tools()
            .find(|t| t.name == call.function_name)
            .and_then(|t| t.classification.clone());
        let classification =
            output.classification.or(configured).unwrap_or_else(|| channel.clone());
        let context = turn.situation().security_context();
        let parts = iter::once(&classification)
            .chain(output.shown.iter().map(|shown| &shown.classification));
        if let Some(refused) = parts
            .into_iter()
            .find(|part| !self.policies.confidentiality().may_publish(context, part, &channel))
        {
            debug!(tool = %call.function_name, classification = %refused, "Withheld a result the channel does not admit");
            return ToolCallResult::of(
                call,
                ToolResultStatus::Withheld,
                withheld_result(&turn.situation().texts(), refused),
                channel,
            );
        }
        match successful_result(&turn.situation().texts(), output.value, call.visibility) {
            Ok(content) => ToolCallResult {
                shown: output.shown,
                ..ToolCallResult::of(call, ToolResultStatus::Ok, content, classification)
            },
            // Not given without the mark that it is the agent's alone.
            Err(e) => {
                error!(tool = %call.function_name, "Withheld a result only its agent may see, which cannot be marked so: {e}");
                ToolCallResult::of(
                    call,
                    ToolResultStatus::Withheld,
                    unsuccessful_result(ToolResultStatus::Withheld, Err(e)),
                    channel,
                )
            }
        }
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
        let agent = turn.situation().agent_id();
        if !offered(call, turn) {
            info!(
                "Tool call '{}' by agent {} names a tool it was not offered",
                call.function_name, agent
            );
            return Admission::Answered(Box::new(self.failed(
                call,
                ToolError::UnknownTool(call.function_name.clone()),
                turn,
            )));
        }
        // The one question of the call's permission (issue 304, rule 4):
        // asked as it is admitted, not again when it runs.
        if self
            .policies
            .access_policy()
            .require(turn.situation().security_context(), ActionType::CallTool(call), |_| ())
            .await
            .is_err()
        {
            info!(
                "Tool call '{}' by agent {} forbidden by the access policy",
                call.function_name, agent
            );
            return Admission::Answered(Box::new(ToolCallResult::of(
                call,
                ToolResultStatus::Forbidden,
                forbidden_result(&turn.situation().texts()),
                turn.situation().label(),
            )));
        }
        let rule = self.place_rule(call);
        if let Some((rule_id, RuleDecision::Deny)) = rule {
            info!(rule = %rule_id, "Tool call '{}' by agent {} denied by a rule", call.function_name, agent);
            return Admission::Answered(Box::new(ToolCallResult::of(
                call,
                ToolResultStatus::Forbidden,
                forbidden_result(&turn.situation().texts()),
                turn.situation().label(),
            )));
        }
        let description = match self.providers.describe(call) {
            Ok(description) => description,
            Err(e @ (ToolError::InvalidParameter(_) | ToolError::UnknownTool(_))) => {
                info!("Tool call '{}' by agent {} cannot run: {}", call.function_name, agent, e);
                return Admission::Answered(Box::new(self.failed(call, e, turn)));
            }
            Err(e) => {
                warn!("Cannot describe tool call '{}' for approval: {}", call.function_name, e);
                "No description".to_string()
            }
        };
        let questions = match self.questions(call, turn) {
            Ok(questions) => questions,
            Err(e) => {
                info!(
                    "Tool call '{}' by agent {} cannot ask its questions: {}",
                    call.function_name, agent, e
                );
                return Admission::Answered(Box::new(self.failed(call, e, turn)));
            }
        };
        let explicit = rule.is_some_and(|(_, decision)| decision == RuleDecision::Ask);
        let asks =
            explicit || turn.tools().any(|t| t.name == call.function_name && t.requires_approval);
        match questions {
            _ if asks => Admission::Ask {
                description,
                explicit,
                suggested: self.providers.matcher(call),
                questions,
            },
            Some(questions) => Admission::Await(questions),
            None => Admission::Run,
        }
    }

    /// The tool sets the agent is offered in `situation` (issues 180, 290): what
    /// each provider offers there, and a set whose destination the work's
    /// channel may not flow into (`ConfidentialityPolicy::may_publish`, in
    /// the name the work acts in) offered as unavailable, without its tools -
    /// its arguments would carry what the channel holds out of the system.
    pub async fn tool_sets(&self, situation: &Situation<'_>) -> Vec<ToolSet> {
        let context = situation.security_context();
        let channel = &situation.classification();
        let mut offered = Vec::new();
        for set in self.providers.tool_sets(situation).await {
            let withheld = match (&set.destination, &set.unavailable) {
                (Some(destination), None) => {
                    !self.policies.confidentiality().may_publish(context, channel, destination)
                }
                _ => false,
            };
            if withheld {
                debug!(set = %set.name, %channel, "Withheld a tool set whose remote side is not cleared for the channel");
            }
            offered.push(if withheld { set.withheld() } else { set });
        }
        offered
    }

    /// Nothing if `call` may be run now: it was offered, the caller is an
    /// agent and no rule of a place denies it - one may have been set while
    /// the call waited. Whether the policy permits it was asked as it was
    /// admitted (`admit`), once (issue 304).
    async fn check(&self, call: &ToolCall, turn: &Turn<'_>) -> ToolResult<()> {
        if !offered(call, turn) {
            return Err(ToolError::UnknownTool(call.function_name.clone()));
        }
        let security = turn.situation().security_context();
        if security.caller_id.as_agent_id().is_none() {
            return Err(ToolError::PermissionDenied(format!(
                "tools can only be called by agents, not by {}",
                security.caller_id
            )));
        }
        // A rule that denies may have been set while the call waited.
        match self.place_rule(call) {
            Some((rule_id, RuleDecision::Deny)) => {
                Err(ToolError::PermissionDenied(format!("rule {rule_id}")))
            }
            _ => Ok(()),
        }
    }
}

/// Registers the tool kinds with `registry`.
pub fn register_all(registry: &mut ComponentRegistry) {
    ask_user_tool::register(registry);
    attachment_tool::register(registry);
    calculator_tool::register(registry);
    echo_tool::register(registry);
    sleep_tool::register(registry);
    stop_tool::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TestAgents};
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::ConfidentialityPolicy;
    use caelumora_model::config::{AppConfig, ApprovalsConfig};
    use caelumora_model::content::testing::make_content_ref;
    use caelumora_model::content::ContentId;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::AccessPolicy;
    use caelumora_runtime::session::testing::detached_actor_runtime_in;
    use caelumora_runtime::testing::TestRuntime;

    use caelumora_model::channel::Channel;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::context::{ContextRef, FrameRef};
    use caelumora_model::note::NoteId;
    use caelumora_model::security::{AllowAllPolicy, OwnershipPolicy, SecurityContext};
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::{Session, SessionId, SessionUpdateEvent};
    use caelumora_model::tool::{ToolCallStatus, ToolResultStatus};
    use caelumora_model::turn::TurnState;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::Name;
    use caelumora_model::work::Work;
    use serde_json::json;
    use std::future;
    use tokio::time;

    /// What a storage of content reported is told alike, whatever it keeps (issue 466): what the
    /// model can correct as a parameter, a storage that failed as the tool's failure.
    #[test]
    fn a_storages_failure_is_the_tools_and_what_does_not_fit_the_models() {
        let corrected = |error: ToolError| matches!(error, ToolError::InvalidParameter(_));
        let failed = |error: ToolError| matches!(error, ToolError::ExecutionFailed(_));
        assert!(corrected(ContentError::TooLarge { limit: 1 }.into()));
        assert!(corrected(ContentError::Changed.into()));
        assert!(failed(ContentError::Unavailable("permission denied".to_string()).into()));
        assert!(
            failed(ContentError::NotFound("attachments/a".to_string()).into()),
            "a key its owner made"
        );
        assert!(
            corrected(AttachmentError::Content(ContentError::TooLarge { limit: 1 }).into()),
            "an attachment's, as the storage's"
        );
        assert!(failed(
            AttachmentError::Content(ContentError::Unavailable("permission denied".to_string()))
                .into()
        ));
        let gone = ToolError::from(AttachmentError::StorageGone(StorageGone("old".to_string())));
        assert!(
            matches!(&gone, ToolError::ExecutionFailed(m) if !m.contains("old")),
            "a storage no longer configured, without its name: {gone}"
        );
        let gone = ToolError::from(WorkspaceError::StorageGone(StorageGone("old".to_string())));
        assert!(
            matches!(&gone, ToolError::ExecutionFailed(m) if !m.contains("old")),
            "a workspace's too: {gone}"
        );
        assert!(
            failed(AttachmentError::Altered(ContentId::new()).into()),
            "other bytes than were kept"
        );
        assert!(corrected(AttachmentError::NotFound(ContentId::new()).into()));
    }

    #[test]
    fn arguments_are_read_as_a_struct_and_what_does_not_fit_is_the_models_to_correct() {
        #[derive(Deserialize, Debug)]
        struct Note {
            text: String,
            #[serde(default)]
            channel: Option<String>,
        }
        #[derive(Deserialize, Debug)]
        struct Optional {
            #[serde(default)]
            channel: Option<String>,
        }

        let note: Note =
            arguments(&json!({ "text": "hello", "extra": 1 })).expect("unknown fields are ignored");
        assert_eq!((note.text.as_str(), note.channel), ("hello", None));
        let Optional { channel } =
            arguments(&Value::Null).expect("no arguments are an empty object");
        assert!(channel.is_none());

        let missing = arguments::<Note>(&json!({})).unwrap_err();
        assert!(
            matches!(&missing, ToolError::InvalidParameter(m) if m.contains("`text`")),
            "serde names what is missing: {missing}"
        );
        let wrong = arguments::<Note>(&json!({ "text": 5 })).unwrap_err();
        assert!(
            matches!(&wrong, ToolError::InvalidParameter(m) if m.contains("invalid type")),
            "{wrong}"
        );
        let id: NoteId = id_argument(&json!({ "id": 3 })).unwrap();
        assert_eq!(id.0, 3);
        assert_eq!(text_argument(&json!({ "text": "  tea " })).unwrap(), "tea", "trimmed");
        assert!(
            matches!(text_argument(&json!({ "text": " " })), Err(ToolError::InvalidParameter(_))),
            "empty is no text"
        );
    }

    /// `test_tool` does not classify its result; `hr_tool` classifies it as
    /// HR's; `configured_tool` does not classify, but is configured as HR's;
    /// `showing_tool` shows an attachment of the channel's, `showing_hr_tool`
    /// one of HR's (issue 282).
    #[derive(Debug)]
    struct MockToolProvider {
        requires_approval: bool,
    }

    fn hr() -> Classification {
        Classification::new(Confidentiality::Internal, [Compartment::new("hr")]).unwrap()
    }

    fn tool(name: &str, requires_approval: bool, classification: Option<Classification>) -> Tool {
        Tool {
            name: name.to_string(),
            description: "desc".to_string(),
            parameters: ToolParameters::object(
                Vec::<(String, ToolParameters)>::new(),
                Vec::<String>::new(),
            ),
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
                tool("showing_tool", self.requires_approval, None),
                tool("showing_hr_tool", self.requires_approval, None),
            ]
        }
        async fn execute(
            &self,
            name: &str,
            _params: &Value,
            _answers: Option<&Answers>,
            _turn: &mut Turn,
        ) -> ToolResult<ToolOutput> {
            match name {
                "hr_tool" => Ok(ToolOutput::classified(json!({"salary": 1}), hr())),
                "configured_tool" => Ok(json!({"salary": 2}).into()),
                "hanging_tool" => future::pending().await,
                "showing_tool" | "showing_hr_tool" => {
                    let classification = if name == "showing_hr_tool" {
                        hr()
                    } else {
                        Classification::CHANNEL_DEFAULT
                    };
                    let shown = ContentRef { classification, ..make_content_ref("a.png", b"a") };
                    Ok(ToolOutput { shown: vec![shown], ..json!({"shown": true}).into() })
                }
                _ => Ok(json!({"result": "ok"}).into()),
            }
        }
    }

    #[derive(Debug)]
    struct DenyAllPolicy;

    #[async_trait]
    impl AccessPolicy for DenyAllPolicy {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: Some("call_1".to_string()),
            function_name: name.to_string(),
            arguments: json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        }
    }

    fn parse(result: &str) -> Value {
        serde_json::from_str(result).expect("an unsuccessful result is a JSON object")
    }

    #[test]
    fn each_error_maps_to_its_own_status_with_a_next_step() {
        let available = vec!["calculator".to_string(), "echo".to_string()];
        let cases = [
            (
                ToolError::UnknownTool("shell".to_string()),
                ToolResultStatus::UnknownTool,
                "Available tools: calculator, echo",
            ),
            (
                ToolError::InvalidParameter("'x' is missing".to_string()),
                ToolResultStatus::InvalidArguments,
                "call the tool again",
            ),
            (
                ToolError::ExecutionFailed("disk full".to_string()),
                ToolResultStatus::Failed,
                "disk full",
            ),
            (
                ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string()),
                ToolResultStatus::Forbidden,
                "not permitted",
            ),
        ];
        for (error, expected, fragment) in cases {
            let (status, result) = error.to_result(&testing::prompts(), &available);
            assert_eq!(status, expected);
            let json = parse(&result);
            assert_eq!(json["status"], serde_json::to_value(expected).unwrap());
            let message = json["message"].as_str().unwrap();
            assert!(message.contains(fragment), "{:?}: {}", expected, message);
        }
    }

    #[test]
    fn a_policy_refusal_does_not_disclose_the_policy() {
        let (_, result) = ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string())
            .to_result(&testing::prompts(), &[]);
        assert!(!result.contains("rule 7"), "{}", result);
        assert_eq!(result, forbidden_result(&testing::prompts()));
    }

    #[test]
    fn denial_and_timeout_are_distinguishable() {
        assert_eq!(parse(&denied_result(&testing::prompts()))["status"], "denied");
        assert_eq!(parse(&timed_out_result(&testing::prompts()))["status"], "timed_out");
        assert!(denied_result(&testing::prompts()).contains("Do not repeat it"));
        assert!(timed_out_result(&testing::prompts()).contains("ask the user whether to try again"));
    }

    fn executor(access_policy: Arc<dyn AccessPolicy>) -> ToolExecutor {
        guarded(access_policy, true, Places::default())
    }

    /// The rules of the places an agent works in (issue 13).
    #[derive(Default)]
    struct Places {
        system: Vec<ApprovalRule>,
        session: Vec<ApprovalRule>,
        channel: Vec<ApprovalRule>,
    }

    /// The mock tools - each requiring approval if `requires_approval` -
    /// guarded by `access_policy` and the rules of `places`; the controllers of
    /// the session and the channel are returned, for their rules to be
    /// changed.
    fn guarded_in(
        access_policy: Arc<dyn AccessPolicy>,
        requires_approval: bool,
        places: Places,
    ) -> (ToolExecutor, Arc<SessionController>, Arc<ChannelController>) {
        guarding(
            access_policy,
            vec![("tools", Arc::new(MockToolProvider { requires_approval }))],
            places,
        )
    }

    /// The tools of `providers`, by their component ids, guarded by
    /// `access_policy` and the rules of `places`.
    fn guarding(
        access_policy: Arc<dyn AccessPolicy>,
        providers: Vec<(&str, Arc<dyn ToolProvider>)>,
        places: Places,
    ) -> (ToolExecutor, Arc<SessionController>, Arc<ChannelController>) {
        guarding_under(access_policy, Arc::new(ConfidentialityRules::default()), providers, places)
    }

    /// As `guarding`, the flow decided by `confidentiality` (issue 304): for
    /// an agent whose runtime holds the policies, the system's rules and the
    /// session of the test (issue 371).
    fn guarding_under(
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        providers: Vec<(&str, Arc<dyn ToolProvider>)>,
        places: Places,
    ) -> (ToolExecutor, Arc<SessionController>, Arc<ChannelController>) {
        let providers = providers
            .into_iter()
            .map(|(id, facet)| ComponentImpl {
                id: id.to_string(),
                fail_policy: Default::default(),
                facet,
            })
            .collect();
        let config =
            AppConfig { approvals: ApprovalsConfig { rules: places.system }, ..Default::default() };
        let session = Session { approval_rules: places.session, ..bare_session(SessionId::new()) };
        let mut channel = Channel::build(
            session.id,
            ActorId::None,
            Name::new("main").unwrap(),
            Default::default(),
            Classification::CHANNEL_DEFAULT,
            None,
        );
        channel.approval_rules = places.channel;
        let runtime = detached_actor_runtime_in(
            TestRuntime {
                access_policy,
                confidentiality,
                config,
                plugins: vec![&TestAgents],
                ..Default::default()
            }
            .build(),
            session,
            channel,
        );
        let session = Arc::clone(runtime.session_controller());
        let channel = Arc::clone(runtime.channel_controller());
        let executor = ToolExecutor::new(providers, runtime).unwrap();
        (executor, session, channel)
    }

    fn guarded(
        access_policy: Arc<dyn AccessPolicy>,
        requires_approval: bool,
        places: Places,
    ) -> ToolExecutor {
        guarded_in(access_policy, requires_approval, places).0
    }

    fn rule(tool: &str, decision: RuleDecision) -> ApprovalRule {
        ApprovalRule::new(
            ToolMatcher { tool: tool.to_string(), arguments: Default::default() },
            decision,
            Default::default(),
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_rule_of_a_place_denies_a_call_or_asks_for_it_explicitly() {
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(AllowAllPolicy::new());
        let admitted = |executor: ToolExecutor| async move {
            let mut agent_channel_state = agent_turn(&executor).await;
            let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
            executor.admit(&call("test_tool"), &turn).await
        };

        let denied = admitted(guarded(
            Arc::clone(&access_policy),
            false,
            Places { system: vec![rule("test_tool", RuleDecision::Deny)], ..Default::default() },
        ))
        .await;
        assert!(
            matches!(denied, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden && r.content == forbidden_result(&testing::prompts())),
            "the system's, as the policy refuses: {denied:?}"
        );
        let asked = admitted(guarded(
            Arc::clone(&access_policy),
            false,
            Places { channel: vec![rule("test_tool", RuleDecision::Ask)], ..Default::default() },
        ))
        .await;
        assert!(
            matches!(asked, Admission::Ask { explicit: true, .. }),
            "the channel's, of a tool that would not ask: {asked:?}"
        );
        let both = admitted(guarded(
            Arc::clone(&access_policy),
            false,
            Places {
                session: vec![rule("*", RuleDecision::Ask)],
                channel: vec![rule("test_tool", RuleDecision::Deny)],
                ..Default::default()
            },
        ))
        .await;
        assert!(
            matches!(both, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden),
            "the strongest of all places: {both:?}"
        );
        let other = admitted(guarded(
            Arc::clone(&access_policy),
            false,
            Places { session: vec![rule("hr_tool", RuleDecision::Deny)], ..Default::default() },
        ))
        .await;
        assert!(matches!(other, Admission::Run), "a rule about another tool: {other:?}");
        let default = admitted(guarded(access_policy, true, Places::default())).await;
        assert!(
            matches!(default, Admission::Ask { explicit: false, .. }),
            "the tool's own default is no rule: {default:?}"
        );
    }

    #[tokio::test]
    async fn a_rule_that_denies_set_while_a_call_waits_refuses_it_when_run() {
        let (executor, session, _channel) =
            guarded_in(Arc::new(AllowAllPolicy::new()), true, Places::default());
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        assert!(matches!(executor.admit(&call("test_tool"), &turn).await, Admission::Ask { .. }));

        // As if set in another instance, and announced.
        let denying =
            SessionUpdateEvent::ApprovalRulesChanged(vec![rule("test_tool", RuleDecision::Deny)]);
        session.process_global_event(&GlobalEvent::updated::<Session>(&session.id(), &denying));
        let result = executor.execute(&call("test_tool"), None, &mut turn, future::pending()).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "as it stands when run");
        assert!(result.started_at.is_none(), "it never ran");
    }

    /// Issue 304, rule 4: the call's permission is asked once, as the call
    /// is admitted - before anyone is asked to approve it -, not again when
    /// it runs.
    #[tokio::test]
    async fn the_policy_is_checked_once_as_the_call_is_admitted() {
        let executor = executor(Arc::new(DenyAllPolicy));

        let agent_id = AgentId::new();
        let mut agent_channel_state = offered(
            &executor,
            Work {
                security_context: SecurityContext {
                    caller_id: ActorId::Agent(agent_id),
                    ..SecurityContext::default()
                },
                ..Default::default()
            },
        )
        .await;
        let mut context = Turn::under_way(agent_id, &mut agent_channel_state);

        let admitted = executor.admit(&call("test_tool"), &context).await;
        assert!(
            matches!(admitted, Admission::Answered(ref r) if r.status == ToolResultStatus::Forbidden && r.content == forbidden_result(&testing::prompts())),
            "refused before anyone is asked: {admitted:?}"
        );
        let result =
            executor.execute(&call("test_tool"), None, &mut context, future::pending()).await;
        assert_eq!(
            result.status,
            ToolResultStatus::Ok,
            "running asks the policy nothing more: only an admitted call is run"
        );
    }

    /// A rule is offered only where one can be kept (issue 400): of a call
    /// with a value too long even to be compared by equality, none.
    #[tokio::test]
    async fn a_call_is_offered_a_rule_only_where_one_can_be_kept() {
        use caelumora_model::approval::{ArgumentMatcher, MAX_EQUALS_CHARS, MAX_PATTERN_CHARS};

        let asking = executor(Arc::new(AllowAllPolicy::new()));
        let mut agent_channel_state = agent_turn(&asking).await;
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let with =
            |text: String| ToolCall { arguments: json!({ "text": text }), ..call("test_tool") };
        let suggested = |admitted| match admitted {
            Admission::Ask { suggested, .. } => suggested,
            other => panic!("its tool requires approval, got {other:?}"),
        };
        let long = suggested(asking.admit(&with("x".repeat(MAX_PATTERN_CHARS + 1)), &turn).await)
            .expect("a rule by equality");
        assert!(matches!(long.arguments["text"], ArgumentMatcher::Equals { .. }), "{long:?}");
        assert!(
            suggested(asking.admit(&with("x".repeat(MAX_EQUALS_CHARS + 1)), &turn).await).is_none(),
            "no rule that cannot be kept"
        );
    }

    #[tokio::test]
    async fn a_call_is_asked_for_run_or_answered_at_once_as_it_can_succeed() {
        let asking = executor(Arc::new(AllowAllPolicy::new()));
        let mut agent_channel_state = agent_turn(&asking).await;
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        assert!(
            matches!(asking.admit(&call("test_tool"), &turn).await, Admission::Ask { description, explicit: false, .. } if description == "desc"),
            "its tool requires approval"
        );
        match asking.admit(&call("does_not_exist"), &turn).await {
            Admission::Answered(result) => {
                assert_eq!(result.status, ToolResultStatus::UnknownTool);
                assert!(
                    result.content.contains("Available tools: test_tool"),
                    "the tools the turn began with: {}",
                    result.content
                );
            }
            other => panic!("an unknown tool is answered at once, got {other:?}"),
        }

        let refusing = executor(Arc::new(DenyAllPolicy));
        let mut agent_channel_state = agent_turn(&refusing).await;
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        assert!(
            matches!(refusing.admit(&call("test_tool"), &turn).await, Admission::Answered(r) if r.status == ToolResultStatus::Forbidden),
            "first the policy"
        );

        let running = guarded(Arc::new(AllowAllPolicy::new()), false, Places::default());
        let mut agent_channel_state = agent_turn(&running).await;
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let admitted = running.admit(&call("test_tool"), &turn).await;
        assert!(matches!(admitted, Admission::Run), "no approval needed");
        let state = admitted.state(call("test_tool"));
        assert!(matches!(state.status, ToolCallStatus::Ready { answers: None }), "to be run");
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
        async fn execute(
            &self,
            _: &str,
            _: &Value,
            _: Option<&Answers>,
            _: &mut Turn,
        ) -> ToolResult<ToolOutput> {
            Ok(json!({ "sent": true }).into())
        }
    }

    #[tokio::test]
    async fn a_set_whose_remote_side_the_channel_may_not_flow_to_is_withheld_and_its_tools_not_called(
    ) {
        use caelumora_model::confidentiality::Confidentiality;
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let outward: Arc<dyn ToolProvider> =
            Arc::new(Outward { destination: Classification::general(Confidentiality::Internal) });
        let local: Arc<dyn ToolProvider> = Arc::new(MockToolProvider { requires_approval: false });
        let (executor, _, _) = guarding(
            access_policy,
            vec![("outward", outward), ("local", local)],
            Places::default(),
        );
        let agent = SecurityContext {
            caller_id: ActorId::Agent(AgentId::new()),
            ..SecurityContext::default()
        };

        let internal = sets_offered(
            &executor,
            Work {
                security_context: agent.clone(),
                classification: Classification::general(Confidentiality::Internal),
                ..Default::default()
            },
        )
        .await;
        assert!(
            internal.iter().all(|set| set.unavailable.is_none() && !set.tools.is_empty()),
            "offered where the channel flows to it: {internal:?}"
        );
        assert_eq!(
            internal[0].destination,
            Some(Classification::general(Confidentiality::Internal)),
            "and says where"
        );

        let confidential = sets_offered(
            &executor,
            Work {
                security_context: agent.clone(),
                classification: Classification::general(Confidentiality::Confidential),
                ..Default::default()
            },
        )
        .await;
        let (outward, local) = (&confidential[0], &confidential[1]);
        assert!(
            outward.tools.is_empty()
                && outward
                    .unavailable
                    .as_deref()
                    .is_some_and(|why| why.contains("not cleared for this channel")),
            "withheld above it: {outward:?}"
        );
        assert!(
            local.unavailable.is_none() && !local.tools.is_empty(),
            "a set that sends nothing out is offered"
        );

        let mut agent_channel_state = offering(
            Work {
                security_context: agent,
                classification: Classification::general(Confidentiality::Confidential),
                ..Default::default()
            },
            confidential,
        );
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let admitted = executor.admit(&call("send_out"), &turn).await;
        assert!(
            matches!(&admitted, Admission::Answered(r) if r.status == ToolResultStatus::UnknownTool),
            "not called by its name: {admitted:?}"
        );
        let result = executor.execute(&call("send_out"), None, &mut turn, future::pending()).await;
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
        for (approval, asks) in [
            (ApprovalMode::Always, [true, true]),
            (ApprovalMode::Never, [false, false]),
            (ApprovalMode::UnlessReadOnly, [false, true]),
        ] {
            assert_eq!(
                [true, false].map(|read_only| approval.required(read_only)),
                asks,
                "{approval:?}: reading, writing"
            );
        }
    }

    #[tokio::test]
    async fn only_an_agent_calls_tools() {
        let executor = executor(Arc::new(AllowAllPolicy::new()));
        let mut agent_channel_state = offered(
            &executor,
            Work {
                security_context: SecurityContext {
                    caller_id: ActorId::User(UserId::new()),
                    ..SecurityContext::default()
                },
                ..Default::default()
            },
        )
        .await;
        let mut context = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        let result =
            executor.execute(&call("test_tool"), None, &mut context, future::pending()).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "whatever the policy says");
    }

    /// A turn of an agent in a general internal channel, with the tools of
    /// `executor` as it began with them.
    async fn agent_turn(executor: &ToolExecutor) -> AgentChannelState {
        offered(
            executor,
            Work {
                security_context: SecurityContext {
                    caller_id: ActorId::Agent(AgentId::new()),
                    ..SecurityContext::default()
                },
                ..Default::default()
            },
        )
        .await
    }

    /// The tool sets `executor` offers in a turn done as `work` says.
    async fn sets_offered(executor: &ToolExecutor, work: Work) -> Vec<ToolSet> {
        let mut state = offering(work, Vec::new());
        let turn = Turn::under_way(AgentId::new(), &mut state);
        executor.tool_sets(&turn.situation()).await
    }

    /// A turn under way, done as `work` says, offered what `executor`
    /// offers in it.
    async fn offered(executor: &ToolExecutor, work: Work) -> AgentChannelState {
        let tool_sets = sets_offered(executor, work.clone()).await;
        offering(work, tool_sets)
    }

    /// A turn under way, done as `work` says, offered `tool_sets` as it
    /// began with them.
    fn offering(work: Work, tool_sets: Vec<ToolSet>) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let tools = state.keep_offer(tool_sets);
        state.turn = Some(TurnState {
            work: Work {
                context: ContextRef {
                    frame: FrameRef { tools, ..work.context.frame },
                    ..work.context
                },
                ..work
            },
            ..Default::default()
        });
        state
    }

    /// An executor whose flow knows the compartment "hr", and which lets
    /// every action (issue 304: the flow is the policy's apart).
    fn with_hr() -> ToolExecutor {
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules};
        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let tools: Arc<dyn ToolProvider> = Arc::new(MockToolProvider { requires_approval: true });
        guarding_under(
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::new(compartments)),
            vec![("tools", tools)],
            Places::default(),
        )
        .0
    }

    #[tokio::test]
    async fn a_call_runs_is_timed_and_classified_as_the_channel_unless_it_says_otherwise() {
        let executor = with_hr();
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        let result = executor.execute(&call("test_tool"), None, &mut turn, future::pending()).await;
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert_eq!(result.content, "{\"result\":\"ok\"}");
        assert_eq!(result.classification, Classification::CHANNEL_DEFAULT, "the channel's");
        let started_at = result.started_at.expect("a call that ran has its start");
        let finished_at = result.finished_at.expect("and its end");
        assert!(finished_at >= started_at);
        assert!(
            serde_json::to_string(&result).unwrap().contains("\"started_at\""),
            "and it is stored with the history"
        );

        let unknown =
            executor.execute(&call("does_not_exist"), None, &mut turn, future::pending()).await;
        assert_eq!(unknown.status, ToolResultStatus::UnknownTool);
        assert!(unknown.content.contains("Available tools: test_tool"), "{}", unknown.content);
        assert_eq!(
            (unknown.id.as_deref(), unknown.tool_name.as_str()),
            (Some("call_1"), "does_not_exist"),
            "the result is the call's"
        );
        assert!(unknown.started_at.is_none(), "it never ran");
    }

    #[tokio::test]
    async fn a_call_still_running_when_its_turn_ends_is_interrupted_and_counts_as_run() {
        let executor = executor(Arc::new(AllowAllPolicy::new()));
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let ending = async {
            time::sleep(Duration::from_millis(20)).await;
            Interruption::TurnEnded
        };

        let result = executor.execute(&call("hanging_tool"), None, &mut turn, ending).await;
        assert_eq!(
            result.status,
            ToolResultStatus::Interrupted,
            "not waited for once the turn ends"
        );
        assert_eq!(result.content, interrupted_result(&testing::prompts()));
        let started_at = result.started_at.expect("it ran");
        let finished_at = result.finished_at.expect("until it was interrupted");
        assert!(finished_at >= started_at);

        let done = executor
            .execute(&call("test_tool"), None, &mut turn, future::ready(Interruption::TurnEnded))
            .await;
        assert_eq!(done.status, ToolResultStatus::Ok, "a tool that is done has its result");
    }

    #[tokio::test]
    async fn a_call_a_user_interrupts_tells_the_model_who_did_and_that_the_turn_goes_on() {
        let executor = executor(Arc::new(AllowAllPolicy::new()));
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let interrupting = async {
            time::sleep(Duration::from_millis(20)).await;
            Interruption::ByUser("alice".to_string())
        };

        let result = executor.execute(&call("hanging_tool"), None, &mut turn, interrupting).await;
        assert_eq!(result.status, ToolResultStatus::Interrupted);
        let told: Value = serde_json::from_str(&result.content).expect("a status and a message");
        let message = told["message"].as_str().expect("a message");
        assert!(message.starts_with("alice interrupted this tool call after "), "who: {message}");
        assert!(message.contains("your turn goes on"), "that the turn goes on: {message}");
        assert!(result.started_at.is_some() && result.finished_at.is_some(), "it ran");
    }

    #[tokio::test]
    async fn a_result_that_may_not_flow_into_the_channel_never_reaches_the_model() {
        let executor = with_hr();
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        for name in ["hr_tool", "configured_tool"] {
            let result = executor.execute(&call(name), None, &mut turn, future::pending()).await;
            assert_eq!(
                result.status,
                ToolResultStatus::Withheld,
                "{name}: HR's, and the channel is general"
            );
            assert!(!result.content.contains("salary"), "{name}: {}", result.content);
            assert!(
                result.content.contains("internal {hr}"),
                "the model is told why: {}",
                result.content
            );
            assert_eq!(
                result.classification,
                Classification::CHANNEL_DEFAULT,
                "what the model is told is the channel's"
            );
            assert!(result.started_at.is_some(), "it ran");
        }
    }

    /// Issue 282: what a result showed goes with it to the model, and is withheld with it where it
    /// may not flow into the channel.
    #[tokio::test]
    async fn what_a_result_showed_goes_with_it_where_the_channel_admits_it() {
        let executor = with_hr();
        let mut agent_channel_state = agent_turn(&executor).await;
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        let shown =
            executor.execute(&call("showing_tool"), None, &mut turn, future::pending()).await;
        assert_eq!((shown.status, shown.shown.len()), (ToolResultStatus::Ok, 1), "into the result");
        let withheld =
            executor.execute(&call("showing_hr_tool"), None, &mut turn, future::pending()).await;
        assert_eq!(withheld.status, ToolResultStatus::Withheld, "HR's, and the channel is general");
        assert!(withheld.shown.is_empty(), "withheld with the result");
        assert!(
            withheld.content.contains("internal {hr}"),
            "the model is told why: {}",
            withheld.content
        );
    }

    #[tokio::test]
    async fn a_result_classified_by_the_tool_is_kept_where_the_channel_admits_it() {
        let executor = with_hr();
        let mut agent_channel_state = agent_turn(&executor).await;
        if let Some(turn) = agent_channel_state.turn.as_mut() {
            turn.work.classification = hr();
        }
        let mut turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let result = executor.execute(&call("hr_tool"), None, &mut turn, future::pending()).await;
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
        let public =
            successful_result(&testing::prompts(), value.clone(), ToolVisibility::SHOWN).unwrap();
        assert_eq!(parse(&public), value, "passed on unchanged");

        let hidden = parse(
            &successful_result(&testing::prompts(), value.clone(), ToolVisibility::CALL_ONLY)
                .unwrap(),
        );
        assert_eq!(hidden["result"], value);
        assert!(hidden["note"].as_str().unwrap().contains("Do not quote or reveal it"));
        let without_result = parse(
            &successful_result(&testing::prompts(), value.clone(), ToolVisibility::WITHOUT_RESULT)
                .unwrap(),
        );
        assert_eq!(without_result, hidden, "the result is what counts, not the arguments");
    }

    #[test]
    fn a_result_that_is_text_is_given_as_text() {
        let text = json!("1. Collect.\n2. Group.");
        assert_eq!(
            successful_result(&testing::prompts(), text.clone(), ToolVisibility::SHOWN).unwrap(),
            "1. Collect.\n2. Group.",
            "not a JSON string"
        );
        let hidden =
            successful_result(&testing::prompts(), text, ToolVisibility::CALL_ONLY).unwrap();
        assert!(
            hidden.starts_with("1. Collect.\n2. Group.\n\n[")
                && hidden.contains("Do not quote or reveal it"),
            "{hidden}"
        );
    }
}
