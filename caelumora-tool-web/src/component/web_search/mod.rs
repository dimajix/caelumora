//! The component kind `web_search` (issue 544): what it is configured with and built of - the
//! provider it searches with, of those Caelumora knows (`Provider`, Tavily's API in `tavily`), and
//! what a search finds, whoever finds it (`Found`) -; its tool `search_web` is its facet
//! `ToolProvider` (`tool`).

mod tavily;
mod tool;

use crate::{NAMESPACE, USER_AGENT};
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, ToolProvider};
use caelumora_api::component::{ComponentError, ComponentFacets, ComponentKind};
use caelumora_api::component::{LibraryComponents, Params};
use caelumora_api::error::describe_request_error;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_domain::secret::SecretError;
use caelumora_domain::tool::Tool;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// The kind of the component, and the path of its texts in the catalogue.
const WEB_SEARCH_KIND: &str = "web_search";

/// How long a search may take unless the endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// A provider of search, as the params name it: whose API the endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Provider {
    /// Tavily's search API (`https://api.tavily.com`), its key a bearer token.
    Tavily,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebSearchParams {
    /// Whose API the endpoint speaks: `tavily`, the one provider so far.
    provider: Provider,
    /// The provider's API, with its key; its timeout is how long a search may take.
    endpoint: RemoteEndpoint,
    /// Whether calls require approval. Left out, they do not: a search only reads, and what leaves
    /// is its query, which `destination` bounds.
    #[serde(default = "ApprovalMode::never")]
    requires_approval: ApprovalMode,
    /// What the provider is cleared for: where the queries go - and stay, in its logs, and in the
    /// engines it asks in turn. Left out, public.
    #[serde(default)]
    destination: Option<Classification>,
    /// What the tool is for, as its tool set tells it; left out, the catalogue's text.
    #[serde(default)]
    description: Option<String>,
}

impl Params for WebSearchParams {}

/// A page a search found: its title, its URL and what the provider gives of its content.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Found {
    title: String,
    url: String,
    snippet: String,
}

/// Why a search found nothing. The endpoint is named without credentials, query and fragment.
#[derive(Debug, thiserror::Error)]
enum SearchError {
    #[error("the credentials of the search are not available: {0}")]
    Secret(#[from] SecretError),
    #[error("'{endpoint}' could not be asked: {reason}")]
    Failed { endpoint: String, reason: String },
    #[error("'{endpoint}' answered with the status {status}{explanation}")]
    Status { endpoint: String, status: u16, explanation: String },
    #[error("'{endpoint}' gave an answer that cannot be read: {reason}")]
    InvalidAnswer { endpoint: String, reason: String },
}

/// A component of the kind `web_search`: the tool `search_web`, which searches the web with one
/// provider.
#[derive(Debug)]
struct WebSearchComponent {
    http: HttpClient,
    provider: Provider,
    endpoint: RemoteEndpoint,
    destination: Classification,
    description: String,
    tool: Tool,
}

impl WebSearchComponent {
    /// The component configured by `params`, asking with `http`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(
        params: WebSearchParams,
        http: HttpClient,
        prompts: &PromptTexts,
    ) -> Result<Self, PromptError> {
        let tool = tool::search_web(prompts, params.requires_approval.required(true))?;
        let description = match params.description {
            Some(description) => description,
            None => tool_text(prompts, NAMESPACE, WEB_SEARCH_KIND, "description", ())?,
        };
        Ok(Self {
            http,
            provider: params.provider,
            endpoint: params.endpoint,
            destination: params.destination.unwrap_or(Classification::PUBLIC),
            description,
            tool,
        })
    }
}

/// Registers the component kind `web_search`, which writes for the agent's model. A component
/// whose endpoint names no key is refused with `InvalidConfig`: every provider wants one.
pub(super) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(WEB_SEARCH_KIND)
                .params::<WebSearchParams>()
                .provides::<dyn ToolProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let params = WebSearchParams::read(&descriptor.params)?;
            if params.endpoint.auth.is_none() {
                return Err(ComponentError::InvalidConfig(format!(
                    "the endpoint of '{}' names no key in 'auth': its provider wants one",
                    descriptor.id
                )));
            }
            let http = HttpClient::builder()
                .timeout(params.endpoint.timeout_or(DEFAULT_TIMEOUT))
                .user_agent(USER_AGENT)
                .build()
                .map_err(|e| {
                    ComponentError::InvalidConfig(format!(
                        "no HTTP client for '{}': {}",
                        descriptor.id,
                        describe_request_error(e)
                    ))
                })?;
            let component = WebSearchComponent::new(params, http, &prompts)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(component));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::WebToolLibrary;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_kernel::component::testing::{built, descriptor};
    use caelumora_kernel::testing::TestKernel;
    use serde_json::{json, Value};

    /// The params of a search with Tavily and its key, with `more`.
    fn tavily(more: Value) -> Value {
        let mut params = json!({
            "provider": "tavily",
            "endpoint": {
                "url": "https://api.tavily.com",
                "auth": { "type": "bearer", "secret": "TAVILY_KEY" },
            },
        });
        params.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        params
    }

    /// Built of nothing but the library: its tool asks for no approval unless configured, and its
    /// queries go where the provider is cleared for, public unless configured.
    #[tokio::test]
    async fn a_component_is_built_with_its_tool_and_cleared_as_configured() {
        let kernel =
            TestKernel { libraries: vec![&AgentsLibrary, &WebToolLibrary], ..Default::default() }
                .build();
        let search = descriptor("search", WEB_SEARCH_KIND, tavily(json!({})));
        let provider = built::<dyn ToolProvider>(&kernel, search).await.unwrap();
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search_web");
        assert!(!tools[0].requires_approval, "no approval unless configured");
        assert_eq!(provider.destination(), Some(Classification::PUBLIC));
        assert_eq!(provider.description(), "Search the web.");

        let internal = Classification::general(Confidentiality::Internal);
        let cleared = tavily(json!({ "destination": internal, "requires_approval": true }));
        let cleared = built::<dyn ToolProvider>(&kernel, descriptor("c", WEB_SEARCH_KIND, cleared))
            .await
            .unwrap();
        assert_eq!(cleared.destination(), Some(internal));
        assert!(cleared.tools()[0].requires_approval, "as configured");
    }

    /// A provider has to be named, one Caelumora knows, and the endpoint has to name a key.
    #[tokio::test]
    async fn a_search_without_a_known_provider_or_a_key_is_refused() {
        let kernel =
            TestKernel { libraries: vec![&AgentsLibrary, &WebToolLibrary], ..Default::default() }
                .build();
        let endpoint = json!({ "url": "https://api.tavily.com" });
        let refused = [
            json!({ "endpoint": endpoint }),
            json!({ "provider": "google", "endpoint": endpoint }),
            json!({ "provider": "tavily", "endpoint": endpoint }),
        ];
        for params in refused {
            let search = descriptor("search", WEB_SEARCH_KIND, params.clone());
            assert!(built::<dyn ToolProvider>(&kernel, search).await.is_err(), "{params}");
        }
    }
}
