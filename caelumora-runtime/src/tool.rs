use crate::agent_turn::Turn;
use crate::component::ComponentImpl;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityManager};
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolCallState, ToolVisibility, ToolResultStatus};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info, warn};

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
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;

    /// Executes a specific tool by name.
    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput>;
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
    /// Called only for calls the policy permits.
    async fn execute(&self, call: &ToolCall, turn: &mut Turn) -> ToolResult<ToolOutput>;
    fn describe(&self, call: &ToolCall) -> ToolResult<String>;
    /// The tools there are now, for the model and for labelling its calls.
    /// Asked of the providers each time: a provider's tools may change while
    /// the agent runs - an MCP server replaced by a newer one - and caching
    /// them is the provider's (issue 100).
    fn tools(&self) -> Vec<Tool>;
}

/// How a tool call of the model is admitted, before anything runs
/// (`GuardedToolExecutor::admit`, issue 155).
#[derive(Debug)]
pub enum Admission {
    /// A user decides first; they are shown `description`.
    Ask { description: String },
    /// It runs as it is.
    Run,
    /// It cannot succeed and is answered at once, without asking anyone.
    Answered(ToolCallResult),
}

impl Admission {
    /// The state `call` starts with, admitted so.
    pub fn state(self, call: ToolCall) -> ToolCallState {
        match self {
            Admission::Ask { description } => ToolCallState::pending(call, description),
            Admission::Run => ToolCallState::approved(call),
            Admission::Answered(result) => ToolCallState::answered(call, result),
        }
    }
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
#[derive(Debug)]
pub struct GuardedToolExecutor {
    security_manager: Arc<dyn SecurityManager>,
    executor: ComponentImpl<dyn ToolExecutor>,
}

impl GuardedToolExecutor {
    pub fn new(security_manager: Arc<dyn SecurityManager>, executor: ComponentImpl<dyn ToolExecutor>) -> Self {
        Self { security_manager, executor }
    }

    /// Runs `call` if it may be run now, and answers it either way.
    pub async fn execute(&self, call: &ToolCall, turn: &mut Turn<'_>) -> ToolCallResult {
        if let Err(e) = self.check(call, turn).await {
            return self.failed(call, e, turn);
        }
        let started_at = chrono::Utc::now();
        let outcome = self.executor.execute(call, turn).await;
        let finished_at = chrono::Utc::now();
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
    /// out: first the policy (`CallTool`; `execute` checks again), then the
    /// call itself (issue 118). Otherwise a user is asked if its tool - as
    /// the turn began with it (issue 137) - requires approval, and shown
    /// what the tool says of the call.
    pub async fn admit(&self, call: &ToolCall, turn: &Turn<'_>) -> Admission {
        let agent = turn.agent_id();
        if !self.is_permitted(call, turn).await {
            info!("Tool call '{}' by agent {} forbidden by the security manager", call.function_name, agent);
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
        let asks = turn.tools().iter().any(|t| t.name == call.function_name && t.requires_approval);
        if asks { Admission::Ask { description } } else { Admission::Run }
    }

    /// Whether the policy permits `call` in the turn.
    async fn is_permitted(&self, call: &ToolCall, turn: &Turn<'_>) -> bool {
        self.security_manager.may_call_tool(turn.security_context(), call).await
    }

    pub fn tools(&self) -> Vec<Tool> {
        self.executor.tools()
    }

    /// Nothing if `call` may be run now: the caller is an agent and the
    /// policy permits the call.
    async fn check(&self, call: &ToolCall, turn: &Turn<'_>) -> ToolResult<()> {
        let security = turn.security_context();
        if security.caller_id.as_agent_id().is_none() {
            return Err(ToolError::PermissionDenied(format!("tools can only be called by agents, not by {}", security.caller_id)));
        }
        self.security_manager.require(security, ActionType::CallTool(call), || ToolError::PermissionDenied(call.function_name.clone())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_impl::default_tool_executor::DefaultToolExecutor;
    use crate::agent_turn::Turn;
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
        }
    }

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn tools(&self) -> Vec<Tool> {
            vec![
                tool("test_tool", self.requires_approval, None),
                tool("hr_tool", self.requires_approval, None),
                tool("configured_tool", self.requires_approval, Some(hr())),
            ]
        }
        async fn execute(&self, name: &str, _params: &Value, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            match name {
                "hr_tool" => Ok(ToolOutput::classified(json!({"salary": 1}), hr())),
                "configured_tool" => Ok(json!({"salary": 2}).into()),
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
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), visibility: Default::default() }
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
        let provider: Arc<dyn ToolProvider> = Arc::new(MockToolProvider { requires_approval: true });
        let tools = DefaultToolExecutor::new(vec![ComponentImpl { id: "tools".to_string(), fail_policy: Default::default(), facet: provider }]).unwrap();
        GuardedToolExecutor::new(security, ComponentImpl { id: "executor".to_string(), fail_policy: Default::default(), facet: Arc::new(tools) })
    }

    #[tokio::test]
    async fn the_policy_is_checked_before_approval_and_again_on_execution() {
        let executor = executor(Arc::new(DenyAllSecurityManager));

        let mut agent_channel_state = AgentChannelState::default();
        let agent_id = AgentId::new();
        agent_channel_state.turn = Some(caelumora_model::agent::TurnState { security_context: SecurityContext { caller_id: ActorId::Agent(agent_id), ..SecurityContext::default() }, ..Default::default() });
        let mut context = Turn::new(agent_id, "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());

        assert!(!executor.is_permitted(&call("test_tool"), &context).await);
        let result = executor.execute(&call("test_tool"), &mut context).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden);
        assert_eq!(result.content, forbidden_result());
        assert!(result.started_at.is_none() && result.finished_at.is_none(), "a refused call never ran");
    }

    #[tokio::test]
    async fn a_call_is_asked_for_run_or_answered_at_once_as_it_can_succeed() {
        let asking = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = agent_turn(&asking);
        let turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        assert!(matches!(asking.admit(&call("test_tool"), &turn).await, Admission::Ask { description } if description == "desc"), "its tool requires approval");
        match asking.admit(&call("does_not_exist"), &turn).await {
            Admission::Answered(result) => {
                assert_eq!(result.status, ToolResultStatus::UnknownTool);
                assert!(result.content.contains("Available tools: test_tool"), "the tools the turn began with: {}", result.content);
            }
            other => panic!("an unknown tool is answered at once, got {other:?}"),
        }

        let refusing = executor(Arc::new(DenyAllSecurityManager));
        let mut agent_channel_state = agent_turn(&refusing);
        let turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        assert!(matches!(refusing.admit(&call("test_tool"), &turn).await, Admission::Answered(r) if r.status == ToolResultStatus::Forbidden), "first the policy");

        let provider: Arc<dyn ToolProvider> = Arc::new(MockToolProvider { requires_approval: false });
        let tools = DefaultToolExecutor::new(vec![ComponentImpl { id: "tools".to_string(), fail_policy: Default::default(), facet: provider }]).unwrap();
        let running = GuardedToolExecutor::new(Arc::new(DummySecurityManager::new()), ComponentImpl { id: "executor".to_string(), fail_policy: Default::default(), facet: Arc::new(tools) });
        let mut agent_channel_state = agent_turn(&running);
        let turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        let admitted = running.admit(&call("test_tool"), &turn).await;
        assert!(matches!(admitted, Admission::Run), "no approval needed");
        let state = admitted.state(call("test_tool"));
        assert!(state.status == caelumora_model::approval::ApprovalStatus::Approved && state.result.is_none(), "to be run");
    }

    #[tokio::test]
    async fn only_an_agent_calls_tools() {
        let executor = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::agent::TurnState { security_context: SecurityContext { caller_id: ActorId::User(caelumora_model::user::UserId::new()), ..SecurityContext::default() }, ..Default::default() });
        let mut context = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());

        let result = executor.execute(&call("test_tool"), &mut context).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "whatever the policy says");
    }

    /// A turn of an agent in a general internal channel, with the tools of
    /// `executor` as it began with them.
    fn agent_turn(executor: &GuardedToolExecutor) -> AgentChannelState {
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::agent::TurnState {
            security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() },
            tools: executor.tools(),
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
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());

        let result = executor.execute(&call("test_tool"), &mut turn).await;
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert_eq!(result.content, "{\"result\":\"ok\"}");
        assert_eq!(result.classification, Classification::CHANNEL_DEFAULT, "the channel's");
        let (started_at, finished_at) = (result.started_at.expect("a call that ran has its start"), result.finished_at.expect("and its end"));
        assert!(finished_at >= started_at);
        assert!(serde_json::to_string(&result).unwrap().contains("\"started_at\""), "and it is stored with the history");

        let unknown = executor.execute(&call("does_not_exist"), &mut turn).await;
        assert_eq!(unknown.status, ToolResultStatus::UnknownTool);
        assert!(unknown.content.contains("Available tools: test_tool"), "{}", unknown.content);
        assert_eq!((unknown.id.as_deref(), unknown.tool_name.as_str()), (Some("call_1"), "does_not_exist"), "the result is the call's");
        assert!(unknown.started_at.is_none(), "it never ran");
    }

    #[tokio::test]
    async fn a_result_that_may_not_flow_into_the_channel_never_reaches_the_model() {
        let executor = executor(with_hr());
        let mut agent_channel_state = agent_turn(&executor);
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());

        for name in ["hr_tool", "configured_tool"] {
            let result = executor.execute(&call(name), &mut turn).await;
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
            turn.classification = hr();
        }
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        let result = executor.execute(&call("hr_tool"), &mut turn).await;
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
