//! The facet `ToolProvider` of `mcp_server` (issues 32, 126, 161, 188, 194, 246): the tools of
//! the component's MCP server, described as configured or as the server describes itself, and
//! unavailable - with why, in general terms - if the server could not be asked for them.
//!
//! Tools are namespaced as `"{descriptor_id}__{remote_tool_name}"` to avoid collisions with
//! built-in tools or other MCP servers, since an agent's tools are found by name across all its
//! providers. No connection is held open between calls: the tools are discovered when they are
//! first offered, in the name of whoever acts, and again once what was discovered is older than
//! the endpoint's `refresh_secs`, or after the server did not know a tool that was called (issue
//! 126); `execute()` opens a fresh connection per call (the Streamable HTTP transport supports
//! stateless operation), which keeps the component free of any stale-connection state. Its
//! credential it resolves of the secrets of whoever asks it (`Situation::secrets`, issue 516). What
//! a tool answers above the component's bound is kept whole beside a preview (issue 552).

use super::{McpServerComponent, McpServerParams};
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{cut_to_preview, ToolError, ToolOutput, ToolProvider, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::question::Answers;
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_domain::secret::{SecretError, Secrets};
use caelumora_domain::tool::{Tool, ToolContent, ToolParameters, ToolVisibility};
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, ToolAnnotations};
use rmcp::service::{RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::time;
use tracing::warn;

#[derive(Debug, thiserror::Error)]
enum McpError {
    #[error("failed to connect to MCP server at '{0}': {1}")]
    Connect(String, String),
    #[error("failed to list tools from MCP server at '{0}': {1}")]
    ListTools(String, String),
    #[error("MCP operation timed out: {0}")]
    Timeout(String),
    #[error("MCP tool call failed: {0}")]
    ToolCallFailed(String),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

type McpResult<T> = Result<T, McpError>;

impl McpError {
    /// Why the server's tools are unavailable after this error, in general
    /// terms for the model and the user (issue 32) - never the error, which
    /// may name hosts. That is in the log.
    fn unavailable(&self) -> String {
        match self {
            McpError::Connect(..) => "its server cannot be reached",
            McpError::ListTools(..) => "its server did not list its tools",
            McpError::Timeout(_) => "its server did not answer in time",
            McpError::ToolCallFailed(_) => "its server failed",
            McpError::Secret(_) => "its credentials are not available",
        }
        .to_string()
    }
}

/// Whether a tool the server annotated with `annotations` only reads, as
/// it says (issue 246): a tool without `readOnlyHint` does not, as MCP
/// says. That a server says so is trusted only where configured
/// (`ApprovalMode::UnlessReadOnly`): it may be wrong, or lie.
fn read_only(annotations: Option<&ToolAnnotations>) -> bool {
    annotations.and_then(|a| a.read_only_hint).unwrap_or(false)
}

/// Between the component id and the remote name of a tool: an MCP tool is
/// offered as `<component id>__<remote name>`.
const NAME_SEPARATOR: &str = "__";

impl McpServerComponent {
    fn strip_prefix<'a>(&self, name: &'a str) -> ToolResult<&'a str> {
        name.strip_prefix(self.descriptor_id.as_str())
            .and_then(|rest| rest.strip_prefix(NAME_SEPARATOR))
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))
    }

    /// The server's tools and what it says of itself, discovered in
    /// the name `secrets` are bound to, within the endpoint's timeout.
    async fn discover(&self, secrets: &dyn Secrets) -> McpResult<(Vec<Tool>, Option<String>)> {
        let discover = async {
            let auth_header = self.params.endpoint.bearer_token(secrets).await?;
            discover_tools(&self.descriptor_id, &self.params, auth_header).await
        };
        time::timeout(self.params.timeout(), discover).await.unwrap_or_else(|_| {
            Err(McpError::Timeout(format!(
                "Timed out discovering tools from MCP server at '{}'",
                self.params.endpoint.shown()
            )))
        })
    }
}

/// A connection to the MCP server at `endpoint`. Its errors show the
/// endpoint without credentials, also where `rmcp` writes it into its own
/// messages (issue 188).
async fn connect(
    endpoint: &RemoteEndpoint,
    auth_header: Option<String>,
) -> McpResult<RunningService<RoleClient, ()>> {
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone());
    if let Some(token) = auth_header {
        transport_config = transport_config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(transport_config);
    let service = ()
        .serve(transport)
        .await
        .map_err(|e| McpError::Connect(endpoint.shown(), endpoint.redact_in(&e.to_string())))?;
    Ok(service)
}

/// The tools of the MCP server `params` names, namespaced by `descriptor_id`
/// and requiring approval as `params` says (issue 78) - tool by tool, from
/// what the server says of each (issue 246) - with what the server
/// says of itself in its `instructions`, if anything (issue 194).
async fn discover_tools(
    descriptor_id: &str,
    params: &McpServerParams,
    auth_header: Option<String>,
) -> McpResult<(Vec<Tool>, Option<String>)> {
    let endpoint = &params.endpoint;
    let service = connect(endpoint, auth_header).await?;
    let instructions = service
        .peer_info()
        .and_then(|info| info.instructions.clone())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let remote_tools = service
        .list_all_tools()
        .await
        .map_err(|e| McpError::ListTools(endpoint.shown(), endpoint.redact_in(&e.to_string())));
    let _ = service.cancel().await;
    let remote_tools = remote_tools?;

    let tools = remote_tools
        .into_iter()
        .map(|t| {
            let description = t.description.map(|d| d.to_string()).unwrap_or_default();
            let schema = Value::Object((*t.input_schema).clone());
            Tool {
                name: format!("{}{}{}", descriptor_id, NAME_SEPARATOR, t.name),
                description,
                parameters: schema_to_tool_parameters(&schema),
                requires_approval: params
                    .requires_approval
                    .required(read_only(t.annotations.as_ref())),
                visibility: ToolVisibility::SHOWN,
                classification: params.classification.clone(),
                presentation: Default::default(),
            }
        })
        .collect();
    Ok((tools, instructions))
}

/// Best-effort conversion of a JSON-Schema fragment (as used by MCP tool
/// `input_schema`s) into `ToolParameters`. Recognizes object/string/number/
/// array/boolean structurally; anything else (enums, oneOf, tuples, ...)
/// falls back to a string parameter with a description hint, since
/// `ToolParameters` does not model full JSON Schema.
fn schema_to_tool_parameters(schema: &Value) -> ToolParameters {
    let Some(obj) = schema.as_object() else {
        return ToolParameters::string().described("Unrecognized parameter schema");
    };

    let description = obj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

    match obj.get("type").and_then(|v| v.as_str()) {
        Some("object") => {
            let properties = obj
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|props| {
                    props.iter().map(|(k, v)| (k.clone(), schema_to_tool_parameters(v))).collect()
                })
                .unwrap_or_default();
            let required = obj
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            ToolParameters::Object { properties, required }
        }
        Some("array") => {
            let items = obj
                .get("items")
                .map(schema_to_tool_parameters)
                .unwrap_or_else(|| ToolParameters::string().described("Array item"));
            ToolParameters::Array { items: Box::new(items), description }
        }
        Some("string") => ToolParameters::String { description, language: None },
        Some("number") | Some("integer") => ToolParameters::Number { description },
        Some("boolean") => ToolParameters::Boolean { description },
        _ => ToolParameters::String {
            description: Some(
                description.unwrap_or_else(|| "Unrecognized parameter schema".to_string()),
            ),
            language: None,
        },
    }
}

impl McpServerComponent {
    /// What the model of the agent is given of what the tool `name` answered (issue 552). As it
    /// came, up to the component's bound: the structured content if there is any, else the text,
    /// else the blocks of the answer as JSON. Above the bound, measured on that, a preview of the
    /// text, with the text kept as `<component id>.txt` where there is one, and the JSON as
    /// `<component id>.json` - unless the text parses to the same JSON, as the specification
    /// recommends, where it would only be a copy.
    ///
    /// # Errors
    /// `ToolCallFailed` for a failure of the tool, which is not kept: what it says is cut to the
    /// size of a preview where a bound is configured.
    fn output_of(&self, name: &str, result: CallToolResult) -> McpResult<ToolOutput> {
        let keep_whole_above = self.params.keep_whole_above_bytes;
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        let blocks =
            text.is_empty().then(|| serde_json::to_value(&result.content).unwrap_or(Value::Null));

        if result.is_error == Some(true) {
            let message = blocks.map_or(text, |blocks| blocks.to_string());
            let message =
                if keep_whole_above.is_some() { cut_to_preview(message) } else { message };
            return Err(McpError::ToolCallFailed(format!(
                "MCP tool '{}' returned an error: {}",
                name, message
            )));
        }

        let data = result.structured_content.or(blocks);
        let json = data.as_ref().map(Value::to_string);
        let given = json.as_deref().unwrap_or(&text);
        if !keep_whole_above.is_some_and(|above| above.keeps(given)) {
            return Ok(data.unwrap_or(Value::String(text)).into());
        }

        let id = &self.descriptor_id;
        let content = |extension: &str, whole: String| ToolContent {
            name: format!("{id}.{extension}"),
            bytes: whole.into(),
        };
        let copy = serde_json::from_str::<Value>(&text).ok() == data;
        let preview = (!text.is_empty()).then_some(text.as_str());
        let whole_text = preview.map(|text| content("txt", text.to_string()));
        let whole_json = json.filter(|_| !copy).map(|json| content("json", json));
        let files = whole_text.into_iter().chain(whole_json).collect();
        Ok(ToolOutput::kept(preview, files, &self.prompts))
    }
}

#[async_trait]
impl ToolProvider for McpServerComponent {
    fn description(&self) -> String {
        self.params
            .description
            .clone()
            .or_else(|| self.discovered.current().and_then(|discovered| discovered.1.clone()))
            .unwrap_or_else(|| format!("The tools of the MCP server '{}'.", self.descriptor_id))
    }

    /// Its tools as last discovered; none before they were.
    fn tools(&self) -> Vec<Tool> {
        self.discovered.current().map(|discovered| discovered.0.clone()).unwrap_or_default()
    }

    /// Its tools wherever it is asked, discovered again in the name of
    /// whoever acts there if what was discovered is outdated. A server that
    /// cannot be asked leaves the tools discovered before (issue 126), or,
    /// without any, its tool set unavailable (issue 32) - and is asked again
    /// the next time.
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        let secrets = situation.secrets();
        match self.discovered.get(|| self.discover(&*secrets)).await {
            Ok(discovered) => Ok(discovered.0.clone()),
            Err(e) => match self.discovered.current() {
                Some(discovered) => {
                    warn!("MCP tool component '{}' failed to discover its tools again ({}); keeping those discovered before", self.descriptor_id, e);
                    Ok(discovered.0.clone())
                }
                None => {
                    warn!("MCP tool component '{}' failed to connect/discover tools ({}); no tools available", self.descriptor_id, e);
                    Err(e.unavailable())
                }
            },
        }
    }

    fn destination(&self) -> Option<Classification> {
        Some(self.params.destination.clone().unwrap_or(Classification::PUBLIC))
    }

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        let remote_name = self.strip_prefix(name)?.to_string();
        let secrets = turn.situation().secrets();

        let call_result: McpResult<ToolOutput> = async {
            let auth_header = self.params.endpoint.bearer_token(&*secrets).await?;

            let service = connect(&self.params.endpoint, auth_header).await?;
            let mut request = CallToolRequestParams::new(remote_name.clone());
            if let Some(arguments) = params.as_object().cloned() {
                request = request.with_arguments(arguments);
            }
            // An interrupted call is dropped here (issue 161), and `service`
            // with it: rmcp then aborts the request and ends the session in
            // the background - this call's alone - which tells the server
            // as much as `notifications/cancelled` would.
            let call = service.call_tool(request);

            let result = time::timeout(self.params.timeout(), call).await;
            let _ = service.cancel().await;

            let result = result.map_err(|_| McpError::Timeout(format!("MCP tool call to '{}' timed out", name)))?
                .map_err(|e| {
                    // A server that does not know the tool answers so (MCP's
                    // "Unknown tool"): it has others now.
                    if matches!(&e, ServiceError::McpError(error) if error.code == ErrorCode::INVALID_PARAMS) {
                        self.discovered.invalidate();
                    }
                    McpError::ToolCallFailed(format!("MCP tool call to '{}' failed: {}", name, self.params.endpoint.redact_in(&e.to_string())))
                })?;

            self.output_of(name, result)
        }.await;

        call_result.map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_agents::testing::prompts;
    use caelumora_agents::tool::{ApprovalMode, KeepWholeAbove};
    use caelumora_domain::confidentiality::Confidentiality;
    use rmcp::model::ContentBlock;
    use serde_json::json;

    /// The bound of the tests, and the size of a preview below it.
    const BOUND: usize = 8 * 1024;
    const PREVIEW_AT_MOST: usize = 4 * 1024;

    /// The params of a server configured with nothing but its endpoint, and no approval.
    fn unconfigured() -> McpServerParams {
        McpServerParams {
            endpoint: RemoteEndpoint::new("http://example.invalid/mcp"),
            requires_approval: ApprovalMode::Never,
            classification: None,
            description: None,
            destination: None,
            keep_whole_above_bytes: None,
        }
    }

    fn provider() -> McpServerComponent {
        McpServerComponent::new("github-mcp".to_string(), unconfigured(), prompts())
    }

    /// The component `tracker`, which keeps what lies above `BOUND`.
    fn keeping() -> McpServerComponent {
        let keep_whole_above_bytes = Some(KeepWholeAbove::try_from(BOUND).unwrap());
        let params = McpServerParams { keep_whole_above_bytes, ..unconfigured() };
        McpServerComponent::new("tracker".to_string(), params, prompts())
    }

    /// An answer with `text`, if any, and with `structured` content, if any.
    fn answer(text: Option<&str>, structured: Option<Value>) -> CallToolResult {
        let blocks = text.map(ContentBlock::text).into_iter().collect();
        let mut answer = CallToolResult::success(blocks);
        answer.structured_content = structured;
        answer
    }

    /// JSON whose compact form is above `BOUND`.
    fn large_json() -> Value {
        json!({ "items": "x".repeat(BOUND) })
    }

    /// The names and bytes of what `output` keeps.
    fn kept(output: &ToolOutput) -> Vec<(&str, &[u8])> {
        output.files.iter().map(|file| (file.name.as_str(), file.bytes.as_ref())).collect()
    }

    #[test]
    fn below_the_bound_and_without_one_an_answer_is_given_as_it_came() {
        let long = "x".repeat(BOUND + 1);
        let keeping = keeping();
        for (given, value) in [
            (answer(Some("short"), None), json!("short")),
            (answer(Some("summary"), Some(json!({ "a": 1 }))), json!({ "a": 1 })),
            (answer(None, Some(json!({ "a": 1 }))), json!({ "a": 1 })),
            (answer(None, None), json!([])),
        ] {
            let output = keeping.output_of("t", given).unwrap();
            assert_eq!(output, value.into(), "nothing kept below the bound");
        }
        let at = answer(Some(&long[1..]), None);
        assert!(keeping.output_of("t", at).unwrap().files.is_empty(), "at the bound");
        let whole = provider().output_of("t", answer(Some(&long), None)).unwrap();
        assert_eq!(whole, json!(long).into(), "without a bound, whatever its size");
    }

    #[test]
    fn a_text_above_the_bound_is_kept_as_it_came_beside_a_preview() {
        let text = format!("\u{1b}[1m{}", "x".repeat(BOUND));
        let output = keeping().output_of("t", answer(Some(&text), None)).unwrap();
        assert_eq!(kept(&output), [("tracker.txt", text.as_bytes())], "byte for byte");
        assert_eq!(output.value["content"].as_str().map(str::len), Some(PREVIEW_AT_MOST));
        assert!(output.value["next"].as_str().unwrap().contains("attachment"), "{}", output.value);
    }

    /// Text alone would lose the data of a server whose text only sums it up.
    #[test]
    fn structured_content_above_the_bound_is_kept_beside_a_text_that_is_something_else() {
        let data = large_json();
        let output =
            keeping().output_of("t", answer(Some("Found one."), Some(data.clone()))).unwrap();
        let json = data.to_string();
        assert_eq!(
            kept(&output),
            [("tracker.txt", b"Found one.".as_slice()), ("tracker.json", json.as_bytes())]
        );
        assert_eq!(output.value["content"], "Found one.", "the preview is of the text");
    }

    #[test]
    fn a_text_that_is_the_structured_content_again_is_all_that_is_kept() {
        let data = large_json();
        let indented = serde_json::to_string_pretty(&data).unwrap();
        let output = keeping().output_of("t", answer(Some(&indented), Some(data))).unwrap();
        assert_eq!(kept(&output), [("tracker.txt", indented.as_bytes())], "as it was sent");
    }

    #[test]
    fn json_without_a_text_is_kept_without_a_preview() {
        let data = large_json();
        let output = keeping().output_of("t", answer(None, Some(data.clone()))).unwrap();
        assert_eq!(kept(&output), [("tracker.json", data.to_string().as_bytes())]);
        assert!(output.value.get("content").is_none(), "JSON is not cut in the middle");
        assert!(output.value["next"].is_string(), "{}", output.value);
    }

    /// What is neither text nor structured content goes to the model as JSON, and so is bounded.
    #[test]
    fn an_answer_of_other_blocks_above_the_bound_is_kept_as_their_json() {
        let blocks = vec![ContentBlock::image("x".repeat(BOUND), "image/png")];
        let json = serde_json::to_value(&blocks).unwrap().to_string();
        let output = keeping().output_of("t", CallToolResult::success(blocks)).unwrap();
        assert_eq!(kept(&output), [("tracker.json", json.as_bytes())]);
    }

    #[test]
    fn a_failure_is_cut_where_a_bound_is_configured_and_never_kept() {
        let long = "e".repeat(BOUND * 2);
        let failure = || CallToolResult::error(vec![ContentBlock::text(long.as_str())]);
        let Err(cut) = keeping().output_of("t", failure()) else { panic!("it failed") };
        let cut = cut.to_string();
        assert!(cut.len() < BOUND && cut.ends_with("e..."), "{}", cut.len());
        let Err(whole) = provider().output_of("t", failure()) else { panic!("it failed") };
        assert!(whole.to_string().ends_with(&long), "without a bound, as it came");
    }

    #[test]
    fn a_failure_without_a_text_says_its_blocks_as_json() {
        let blocks = vec![ContentBlock::image("x", "image/png")];
        let json = serde_json::to_value(&blocks).unwrap().to_string();
        let result = CallToolResult::error(blocks);
        let Err(failure) = provider().output_of("t", result) else { panic!("it failed") };
        assert!(failure.to_string().ends_with(&json), "{failure}");
    }

    #[tokio::test]
    async fn an_unreachable_server_is_named_without_what_may_carry_a_credential() {
        let endpoint = RemoteEndpoint::new("http://user:secret@127.0.0.1:1/mcp?key=secret");
        let Err(error) = connect(&endpoint, None).await else { panic!("nobody there") };
        let error = error.to_string();
        assert!(error.contains("'http://127.0.0.1:1/mcp'") && !error.contains("secret"), "{error}");
    }

    /// A tool asks for approval as configured: all, none, or all but those the server says only
    /// read.
    #[test]
    fn a_tool_asks_for_approval_as_configured_and_as_its_server_says_it_only_reads() {
        let read_only = ToolAnnotations::new().read_only(true);
        let writes = ToolAnnotations::new().read_only(false);
        let unannotated = ToolAnnotations::new().destructive(false);
        for (approval, asks) in [
            (ApprovalMode::Always, [true, true, true, true]),
            (ApprovalMode::Never, [false; 4]),
            (ApprovalMode::UnlessReadOnly, [false, true, true, true]),
        ] {
            let required = [Some(&read_only), Some(&writes), Some(&unannotated), None]
                .map(|a| approval.required(super::read_only(a)));
            assert_eq!(
                required, asks,
                "{approval:?}: read-only, writing, without readOnlyHint, without annotations"
            );
        }
    }

    #[test]
    fn namespacing_round_trips_through_prefix_stripping() {
        let provider = provider();
        let stripped = provider.strip_prefix("github-mcp__search").unwrap();
        assert_eq!(stripped, "search");
        assert!(provider.strip_prefix("unrelated_tool").is_err());
    }

    /// Issue 180: the server is cleared for what is configured, public unless configured.
    #[test]
    fn the_server_is_cleared_for_what_is_configured_and_public_unless() {
        let provider = provider();
        assert_eq!(provider.destination(), Some(Classification::PUBLIC));
        let internal = Classification::general(Confidentiality::Internal);
        let cleared = McpServerComponent::new(
            "github-mcp".to_string(),
            McpServerParams { destination: Some(internal.clone()), ..provider.params.clone() },
            prompts(),
        );
        assert_eq!(cleared.destination(), Some(internal));
    }

    #[test]
    fn schema_conversion_recognizes_basic_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "number" },
                "verbose": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["query"]
        });

        let params = schema_to_tool_parameters(&schema);
        match params {
            ToolParameters::Object { properties, required } => {
                assert_eq!(required, vec!["query".to_string()]);
                assert!(matches!(properties.get("query"), Some(ToolParameters::String { .. })));
                assert!(matches!(properties.get("limit"), Some(ToolParameters::Number { .. })));
                assert!(matches!(properties.get("verbose"), Some(ToolParameters::Boolean { .. })));
                assert!(matches!(properties.get("tags"), Some(ToolParameters::Array { .. })));
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn schema_conversion_falls_back_to_string_for_unrecognized_schema() {
        let schema = json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] });
        let params = schema_to_tool_parameters(&schema);
        assert!(matches!(params, ToolParameters::String { .. }));
    }
}
