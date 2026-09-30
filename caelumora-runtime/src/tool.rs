use crate::agent_turn::Turn;
use crate::component::ComponentImpl;
use async_trait::async_trait;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityManager};
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolConfidentiality, ToolResultStatus};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;

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
pub fn successful_result(value: Value, confidentiality: ToolConfidentiality) -> String {
    if confidentiality.content.is_channel() {
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
        ToolResultStatus::Failed,
        "This tool call was not executed: the server restarted before it ran. Decide again whether to call it; if its effect matters, check first.",
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
    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<Value>;
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
/// how a tool is run, never whether: the runtime checks the permission
/// before it delegates ([`GuardedToolExecutor`]), so that no configured
/// component can run a call the policy refuses.
#[async_trait]
pub trait ToolExecutor: Send + Sync + Debug {
    /// Runs `call` and answers it - always with a result for the model: what
    /// the tool gave, or what went wrong and what to do next (issue 15).
    /// Called only for calls the policy permits.
    async fn execute(&self, call: &ToolCall, turn: &mut Turn) -> ToolCallResult;
    fn describe(&self, call: &ToolCall) -> ToolResult<String>;
    /// The tools there are now, for the model and for labelling its calls.
    /// Asked of the providers each time: a provider's tools may change while
    /// the agent runs - an MCP server replaced by a newer one - and caching
    /// them is the provider's (issue 100).
    fn tools(&self) -> Vec<Tool>;
}

/// An agent's [`ToolExecutor`] with the checks the runtime keeps (issue
/// 127): only an agent calls tools, and only what the policy permits
/// (`CallTool`) - asked before a call is put up for approval
/// (`is_permitted`) and again when it is run. Whatever executor a
/// personality configures, it only ever sees permitted calls.
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
            let available: Vec<String> = self.tools().into_iter().map(|t| t.name).collect();
            let (status, content) = e.to_result(&available);
            return ToolCallResult::of(call, status, content);
        }
        self.executor.facet.execute(call, turn).await
    }

    /// Whether the policy permits `call` in the turn. Checked before asking
    /// anyone for approval, so that users are not asked about calls that
    /// would be refused anyway; `execute` checks again.
    pub async fn is_permitted(&self, call: &ToolCall, turn: &Turn<'_>) -> bool {
        self.security_manager.may_call_tool(turn.security_context(), call).await
    }

    pub fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        self.executor.facet.describe(call)
    }

    pub fn tools(&self) -> Vec<Tool> {
        self.executor.facet.tools()
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
    use crate::component_impl::default_tool_executor::DefaultToolExecutor;
    use crate::agent_turn::Turn;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};

    use caelumora_model::security::{DummySecurityManager, SecurityContext};
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

    #[derive(Debug)]
    struct MockToolProvider {
        requires_approval: bool,
    }

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "test_tool".to_string(),
                description: "desc".to_string(),
                parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
                requires_approval: self.requires_approval,
                confidentiality: Default::default(),
            }]
        }
        async fn execute(&self, _name: &str, _params: &Value, _turn: &mut Turn) -> ToolResult<Value> {
            Ok(json!({"result": "ok"}))
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
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), confidentiality: Default::default() }
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
    async fn only_an_agent_calls_tools() {
        let executor = executor(Arc::new(DummySecurityManager::new()));
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::agent::TurnState { security_context: SecurityContext { caller_id: ActorId::User(caelumora_model::user::UserId::new()), ..SecurityContext::default() }, ..Default::default() });
        let mut context = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());

        let result = executor.execute(&call("test_tool"), &mut context).await;
        assert_eq!(result.status, ToolResultStatus::Forbidden, "whatever the policy says");
    }

    #[test]
    fn a_result_is_written_without_its_status_when_ok_and_read_back() {
        let result: ToolCallResult = serde_json::from_value(json!({"id": "1", "tool_name": "echo", "content": "hi"})).unwrap();
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert!(!serde_json::to_string(&result).unwrap().contains("status"), "Ok is not written");

        let denied = ToolCallResult { status: ToolResultStatus::Denied, ..result };
        assert!(serde_json::to_string(&denied).unwrap().contains("\"status\":\"denied\""));
    }

    #[test]
    fn a_result_the_channel_does_not_see_is_marked_for_the_model() {
        let value = json!({"secret": 42});
        let public = successful_result(value.clone(), ToolConfidentiality::PUBLIC);
        assert_eq!(parse(&public), value, "passed on unchanged");

        let hidden = parse(&successful_result(value.clone(), ToolConfidentiality::CALL_ONLY));
        assert_eq!(hidden["result"], value);
        assert!(hidden["note"].as_str().unwrap().contains("Do not quote or reveal it"));
    }
}
