//! The component kind `openapi_server` (issue 252): what it is configured with - one server
//! described by an OpenAPI 3.x document, where the document lies, which of its operations are
//! offered, how they are approved and classified, what the server is cleared for -; the document
//! is read by `document`, the tools of its operations and their calls are `tool`'s, and their
//! `ToolProvider` is a `DiscoveredTools` over the server.

mod document;
mod tool;

use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{ApprovalMode, DiscoveredTools, KeepWholeAbove, ToolProvider};
use caelumora_api::cache::ExpiringCache;
use caelumora_api::component::{
    ComponentError, ComponentFacets, ComponentKind, ComponentResult, LibraryComponents, Params,
};
use caelumora_api::error::describe_request_error;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::PromptTexts;
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_domain::tool::ToolDescription;
use document::Operation;
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The kind of the component.
pub const OPENAPI_SERVER_KIND: &str = "openapi_server";

/// How long a call or reading the document may take unless the endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a document read of the server is kept before it is read again, unless the endpoint
/// says otherwise.
const DEFAULT_REFRESH: Duration = Duration::from_secs(300);

/// Where the document lies below the endpoint unless `spec_path` or `spec_file` says otherwise.
const DEFAULT_SPEC_PATH: &str = "openapi.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenApiServerParams {
    /// The server: the URL its operations' paths lie below. Its timeout is how long a call, or
    /// reading the document, may take; its `refresh_secs` how long a document read of the server
    /// is kept.
    endpoint: RemoteEndpoint,
    /// Where the document lies below the endpoint, read with its credential; left out, at
    /// `openapi.json` - unless `spec_file` is written, which excludes it.
    #[serde(default)]
    spec_path: Option<String>,
    /// A file that holds the document, relative to the working directory, for a server that does
    /// not serve its document as such; read once.
    #[serde(default)]
    spec_file: Option<PathBuf>,
    /// The operations offered, each by its `operationId`, with what holds for it alone: only these
    /// become tools.
    operations: BTreeMap<String, OperationParams>,
    /// Whether calls of the operations require human approval, unless an operation says so of its
    /// own: left out, they do; `"unless_read_only"` trusts `GET` and `HEAD`.
    #[serde(default)]
    requires_approval: ApprovalMode,
    /// How the results of the operations are classified (issue 133); left out, as the channel.
    #[serde(default)]
    classification: Option<Classification>,
    /// What the operations are for, written for a model: the description of their tool set. Left
    /// out, the document's `info.description`, else a text naming the component.
    #[serde(default)]
    description: Option<String>,
    /// What the server is cleared for (issue 180): where the arguments of its calls go. Left out,
    /// public.
    #[serde(default)]
    destination: Option<Classification>,
    /// From how many bytes on what an operation answers is kept whole, and a preview given in its
    /// stead; left out, nothing is (issue 552).
    #[serde(default)]
    keep_whole_above_bytes: Option<KeepWholeAbove>,
}

/// What holds for one operation alone, over what the component says of all.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationParams {
    /// Whether its calls require approval, whatever `requires_approval` of the component says.
    #[serde(default)]
    requires_approval: Option<bool>,
}

/// Where the document of the server lies.
#[derive(Debug)]
enum DocumentSource<'a> {
    /// Below the endpoint, at this path.
    Served(&'a str),
    /// In this file.
    File(&'a Path),
}

impl Params for OpenApiServerParams {
    fn check(&self) -> ComponentResult<()> {
        let invalid = |why: &str| Err(ComponentError::InvalidConfig(why.to_string()));
        if self.spec_path.is_some() && self.spec_file.is_some() {
            return invalid("spec_path and spec_file exclude each other");
        }
        if self.spec_file.is_some() && self.endpoint.refresh_secs.is_some() {
            return invalid("endpoint.refresh_secs is spec_path's alone: a spec_file is read once");
        }
        if self.operations.is_empty() {
            return invalid("operations must name at least one operation");
        }
        Ok(())
    }
}

impl OpenApiServerParams {
    fn timeout(&self) -> Duration {
        self.endpoint.timeout_or(DEFAULT_TIMEOUT)
    }

    /// Where the document lies.
    fn document(&self) -> DocumentSource<'_> {
        match (&self.spec_file, &self.spec_path) {
            (Some(file), _) => DocumentSource::File(file),
            (None, path) => DocumentSource::Served(path.as_deref().unwrap_or(DEFAULT_SPEC_PATH)),
        }
    }

    /// How long a document read is kept: a file's for ever, as it is read once.
    fn refresh(&self) -> Duration {
        match self.document() {
            DocumentSource::Served(_) => self.endpoint.refresh_or(DEFAULT_REFRESH),
            DocumentSource::File(_) => Duration::MAX,
        }
    }
}

/// What was read of the server's document.
#[derive(Debug)]
struct Discovered {
    /// Each operation chosen, with its description as it is offered.
    operations: Vec<(Arc<Operation>, ToolDescription)>,
    /// What the document says of the server, if it does.
    described: Option<String>,
}

/// One server described by OpenAPI as the component's tools and each of them reach it: how it is
/// configured, the client its requests go by, what was read of its document, and the texts its
/// results give.
#[derive(Debug)]
struct OpenApiServer {
    descriptor_id: String,
    params: OpenApiServerParams,
    http: HttpClient,
    /// What was last read of the document.
    discovered: ExpiringCache<Discovered>,
    /// The texts its results give.
    prompts: PromptTexts,
}

impl OpenApiServer {
    /// The server of the component `descriptor_id` configured by `params`, its texts from
    /// `prompts`; nothing is read of it yet.
    ///
    /// # Errors
    /// `InvalidConfig` if no HTTP client can be made.
    fn new(
        descriptor_id: String,
        params: OpenApiServerParams,
        prompts: PromptTexts,
    ) -> ComponentResult<Self> {
        let http = HttpClient::builder().build().map_err(|e| {
            let shown = params.endpoint.shown();
            let why = describe_request_error(e);
            ComponentError::InvalidConfig(format!("no HTTP client for '{shown}': {why}"))
        })?;
        let discovered = ExpiringCache::new(params.refresh());
        Ok(Self { descriptor_id, params, http, discovered, prompts })
    }
}

/// The tools of the component `descriptor_id` configured by `params`, its texts from `prompts`:
/// described as configured, else as the document says of the server, else by a text naming the
/// component, and cleared for its `destination`, public unless configured; nothing is read yet.
///
/// # Errors
/// `InvalidConfig` if no HTTP client can be made.
fn provider(
    descriptor_id: String,
    params: OpenApiServerParams,
    prompts: PromptTexts,
) -> ComponentResult<DiscoveredTools<OpenApiServer>> {
    let description = params.description.clone();
    let unnamed = format!("The operations of the OpenAPI server '{descriptor_id}'.");
    let destination = params.destination.clone().unwrap_or(Classification::PUBLIC);
    let server = Arc::new(OpenApiServer::new(descriptor_id.clone(), params, prompts)?);
    Ok(DiscoveredTools::new(descriptor_id, server, description, unnamed, destination))
}

/// Registers the component kind `openapi_server`, which writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(OPENAPI_SERVER_KIND)
                .params::<OpenApiServerParams>()
                .reaches()
                .provides::<dyn ToolProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let params = OpenApiServerParams::read(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            let provider = provider(descriptor.id.clone(), params, prompts)?;
            facets.insert::<dyn ToolProvider>(Arc::new(provider));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_agents::testing::prompts;
    use caelumora_domain::confidentiality::Confidentiality;
    use serde_json::{json, Value};

    /// `params` read, or why not.
    fn read(params: Value) -> ComponentResult<OpenApiServerParams> {
        OpenApiServerParams::read(&params)
    }

    #[test]
    fn the_document_lies_below_the_endpoint_unless_a_file_holds_it() {
        let endpoint = json!({ "url": "https://kimai.example" });
        let operations = json!({ "get_timesheets": {} });
        let unwritten = read(json!({ "endpoint": endpoint, "operations": operations })).unwrap();
        assert!(
            matches!(unwritten.document(), DocumentSource::Served("openapi.json")),
            "by default"
        );
        assert_eq!(unwritten.refresh(), Duration::from_secs(300));
        let served =
            json!({ "endpoint": endpoint, "operations": operations, "spec_path": "a.json" });
        assert!(matches!(read(served).unwrap().document(), DocumentSource::Served("a.json")));
        let file = json!({ "endpoint": endpoint, "operations": operations, "spec_file": "k.json" });
        let file = read(file).unwrap();
        assert!(
            matches!(file.document(), DocumentSource::File(path) if path == Path::new("k.json"))
        );
        assert_eq!(file.refresh(), Duration::MAX, "read once");
    }

    #[test]
    fn params_that_contradict_themselves_or_offer_nothing_are_refused() {
        let refused = |params: Value| read(params).unwrap_err().to_string();
        let endpoint = json!({ "url": "https://kimai.example" });
        let operations = json!({ "get_timesheets": {} });
        let both = json!({
            "endpoint": endpoint, "operations": operations,
            "spec_path": "a.json", "spec_file": "a.json"
        });
        assert!(refused(both).contains("exclude each other"));
        let refreshed = json!({
            "endpoint": { "url": "https://kimai.example", "refresh_secs": 60 },
            "operations": operations, "spec_file": "a.json"
        });
        assert!(refused(refreshed).contains("refresh_secs"));
        let none = json!({ "endpoint": endpoint, "operations": {} });
        assert!(refused(none).contains("at least one operation"));
        let unwritten = refused(json!({ "endpoint": endpoint }));
        assert!(unwritten.contains("operations"), "required: {unwritten}");
        let unknown = json!({ "endpoint": endpoint, "operations": { "a": { "approval": true } } });
        assert!(read(unknown).is_err(), "nothing unknown in an operation either");
    }

    #[test]
    fn approval_is_required_unless_configured_for_all_or_for_an_operation() {
        let params = read(json!({
            "endpoint": { "url": "https://kimai.example" },
            "operations": {
                "get_timesheets": {}, "patch_stop_timesheet": { "requires_approval": false }
            },
        }))
        .unwrap();
        assert_eq!(params.requires_approval, ApprovalMode::Always);
        let own = |id: &str| params.operations[id].requires_approval;
        assert_eq!([own("get_timesheets"), own("patch_stop_timesheet")], [None, Some(false)]);
    }

    /// Issue 180: the server is cleared for what is configured, public unless configured.
    #[test]
    fn the_server_is_cleared_for_what_is_configured_and_public_unless() {
        let cleared = |destination: Option<Value>| {
            let mut params = json!({
                "endpoint": { "url": "https://kimai.example" }, "operations": { "a": {} }
            });
            if let Some(destination) = destination {
                params["destination"] = destination;
            }
            let params = read(params).unwrap();
            provider("kimai".to_string(), params, prompts()).unwrap().destination()
        };
        assert_eq!(cleared(None), Some(Classification::PUBLIC));
        let confidential = Classification::general(Confidentiality::Confidential);
        assert_eq!(cleared(Some(json!({ "level": "confidential" }))), Some(confidential));
    }
}
