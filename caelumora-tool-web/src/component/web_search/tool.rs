//! The facet `ToolProvider` of `web_search` (issue 544): the tool `search_web`, which searches the
//! web with the component's provider and gives what it found - title, URL and snippet of each
//! page, and nothing of the query -, public as the web it found it on. Its key it resolves of the
//! secrets of whoever asks it (`Situation::secrets`, issue 516).

use super::{tavily, Found, Provider, SearchError, WebSearchComponent, WEB_SEARCH_KIND};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::tool::{
    arguments, described, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::secret::Secrets;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::{json, Value};

/// The one tool of the component.
const SEARCH_WEB: &str = "search_web";

/// The web searched: its `query`, and the `results` of its result - each `title`, `url` and
/// `snippet`.
const WEB_RESULTS_PRESENTATION: ToolPresentation = ToolPresentation::new("web_results");

/// How many pages a search finds unless the call says otherwise.
const DEFAULT_RESULTS: usize = 5;

/// How many pages a search finds at most.
const MAX_RESULTS: usize = 20;

/// The arguments of a call of `search_web`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

/// The tool `search_web`, its texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
pub(super) fn search_web(
    prompts: &PromptTexts,
    requires_approval: bool,
) -> Result<Tool, PromptError> {
    let tool = Tool {
        name: SEARCH_WEB.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [("query", ToolParameters::string()), ("max_results", ToolParameters::number())],
            ["query"],
        ),
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: WEB_RESULTS_PRESENTATION,
    };
    let values = json!({ "default_results": DEFAULT_RESULTS, "max_results": MAX_RESULTS });
    described(prompts, NAMESPACE, WEB_SEARCH_KIND, tool, values)
}

impl WebSearchComponent {
    /// What the provider finds for `query`, at most `max_results` pages, asked with the key the
    /// endpoint names, resolved of `secrets`.
    ///
    /// # Errors
    /// `Secret` where the key cannot be resolved, else what the provider's search fails with.
    async fn search(
        &self,
        secrets: &dyn Secrets,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<Found>, SearchError> {
        // `register` builds no component whose endpoint names no key.
        let key =
            self.endpoint.bearer_token(secrets).await?.ok_or_else(|| SearchError::Failed {
                endpoint: self.endpoint.shown(),
                reason: "its endpoint names no key".to_string(),
            })?;
        match self.provider {
            Provider::Tavily => {
                tavily::search(&self.http, &self.endpoint, &key, query, max_results).await
            }
        }
    }
}

/// What the model is told of a search that found nothing: a query the provider refused as it is
/// written is its to correct; anything else is the call's failure.
impl From<SearchError> for ToolError {
    fn from(error: SearchError) -> Self {
        match error {
            SearchError::Status { status: 400, .. } => {
                ToolError::InvalidParameter(error.to_string())
            }
            _ => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

#[async_trait]
impl ToolProvider for WebSearchComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![self.tool.clone()]
    }

    fn destination(&self) -> Option<Classification> {
        Some(self.destination.clone())
    }

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        if name != SEARCH_WEB {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let Search { query, max_results } = arguments(params)?;
        if query.trim().is_empty() {
            return Err(ToolError::InvalidParameter("the query is empty".to_string()));
        }
        let max_results = max_results.unwrap_or(DEFAULT_RESULTS);
        if !(1..=MAX_RESULTS).contains(&max_results) {
            return Err(ToolError::InvalidParameter(format!(
                "max_results is {max_results}; a search finds from 1 to {MAX_RESULTS} pages"
            )));
        }
        let secrets = turn.situation().secrets();
        let found = self.search(&*secrets, &query, max_results).await?;
        // The query is not repeated: it is the agent's, classified as its channel, and the result
        // holds what the web gave alone.
        Ok(ToolOutput::classified(json!({ "results": found }), Classification::PUBLIC))
    }
}

#[cfg(test)]
mod tests {
    use super::super::WebSearchParams;
    use super::*;
    use crate::testing::prompts;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::{Json, Router};
    use caelumora_agents::testing::detached_actor_runtime;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::remote::RemoteEndpoint;
    use caelumora_domain::secret::SecretResult;
    use caelumora_domain::work::Work;
    use caelumora_security::secret::{SecretManager, SecretProvider};
    use caelumora_security::{AllowAllPolicy, SecurityContext};
    use reqwest::Client as HttpClient;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    /// The one secret of the tests: Tavily's key.
    #[derive(Debug)]
    struct Key;

    #[async_trait]
    impl SecretProvider for Key {
        async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
            Ok((key == "TAVILY_KEY").then(|| "tvly-secret".to_string()))
        }
    }

    fn secrets() -> Arc<dyn Secrets> {
        let manager =
            Arc::new(SecretManager::new(vec![Arc::new(Key)], Arc::new(AllowAllPolicy::new())));
        Arc::new(manager.for_context(SecurityContext::default()))
    }

    /// What the stand-in of Tavily was asked: the header `authorization` and the body of each
    /// request.
    type Asked = Arc<Mutex<Vec<(Option<String>, Value)>>>;

    /// A stand-in of Tavily on the host itself, which finds two pages - or, for the query
    /// `refused`, answers 401, for `too long` 400, as Tavily does; its endpoint, and what it was
    /// asked.
    async fn tavily() -> (RemoteEndpoint, Asked) {
        async fn search(
            State(asked): State<Asked>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Response {
            let authorization =
                headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
            asked.lock().unwrap().push((authorization, body.clone()));
            if body["query"] == "refused" {
                let detail = json!({
                    "detail": { "error": "Unauthorized: missing or invalid API key." }
                });
                return (StatusCode::UNAUTHORIZED, Json(detail)).into_response();
            }
            if body["query"] == "too long" {
                let detail = json!({ "detail": { "error": "Query is too long." } });
                return (StatusCode::BAD_REQUEST, Json(detail)).into_response();
            }
            Json(json!({
                "query": body["query"],
                "results": [
                    {
                        "title": "Tokio",
                        "url": "https://tokio.rs/",
                        "content": "An asynchronous runtime",
                        "score": 0.9,
                        "raw_content": null,
                    },
                    { "title": null, "url": "https://docs.rs/tokio", "content": "API docs" },
                    { "title": "No URL", "url": null, "content": "left out" },
                ],
                "response_time": 0.5,
            }))
            .into_response()
        }
        let asked = Asked::default();
        let router = Router::new().route("/search", post(search)).with_state(Arc::clone(&asked));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let endpoint = serde_json::from_value(json!({
            "url": format!("http://127.0.0.1:{port}"),
            "auth": { "type": "bearer", "secret": "TAVILY_KEY" },
        }))
        .unwrap();
        (endpoint, asked)
    }

    /// The component searching with a stand-in of Tavily of its own.
    async fn component_of_tavily() -> WebSearchComponent {
        component(tavily().await.0)
    }

    /// The component searching with Tavily at `endpoint`.
    fn component(endpoint: RemoteEndpoint) -> WebSearchComponent {
        let params = WebSearchParams {
            provider: Provider::Tavily,
            endpoint,
            requires_approval: Default::default(),
            destination: None,
            description: None,
        };
        WebSearchComponent::new(
            params,
            HttpClient::builder().no_proxy().build().unwrap(),
            &prompts(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_search_asks_tavily_with_the_key_for_results_alone_and_gives_the_pages_it_found() {
        let (endpoint, asked) = tavily().await;
        let component = component(endpoint);
        assert_eq!(component.tools()[0].presentation, WEB_RESULTS_PRESENTATION, "shown as found");
        let found = component.search(&*secrets(), "tokio", 2).await.unwrap();
        assert_eq!(
            found,
            [
                Found {
                    title: "Tokio".into(),
                    url: "https://tokio.rs/".into(),
                    snippet: "An asynchronous runtime".into()
                },
                Found {
                    title: String::new(),
                    url: "https://docs.rs/tokio".into(),
                    snippet: "API docs".into()
                },
            ]
        );
        let asked = asked.lock().unwrap();
        let (authorization, body) = &asked[0];
        assert_eq!(authorization.as_deref(), Some("Bearer tvly-secret"));
        assert_eq!(body["query"], "tokio");
        assert_eq!(body["max_results"], 2);
        assert_eq!(body["include_answer"], false, "no answer of its own");
        assert_eq!(body["include_raw_content"], false, "no content beyond a snippet");
    }

    #[tokio::test]
    async fn a_refused_search_names_the_endpoint_and_what_tavily_says_never_the_key() {
        let (endpoint, _) = tavily().await;
        let error = component(endpoint).search(&*secrets(), "refused", 5).await.unwrap_err();
        let error = error.to_string();
        assert!(error.contains("status 401") && error.contains("invalid API key"), "{error}");
        assert!(error.contains("'http://127.0.0.1:") && !error.contains("tvly-secret"), "{error}");
        let too_long = component_of_tavily().await.search(&*secrets(), "too long", 5).await;
        let too_long = ToolError::from(too_long.unwrap_err());
        assert!(matches!(too_long, ToolError::InvalidParameter(_)), "the model's to correct");
    }

    /// A call that asks for no page, for more than a search finds, or for nothing is the model's
    /// to correct, before anything is asked; the key is resolved in the name of whoever acts.
    #[tokio::test]
    async fn a_call_asks_for_at_least_one_page_and_at_most_the_most() {
        let (endpoint, asked) = tavily().await;
        let component = component(endpoint);
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            Default::default(),
            None,
            Default::default(),
            Default::default(),
        ));
        let mut turn = Turn::of(&mut state, detached_actor_runtime(AgentId::new())).unwrap();
        for params in [
            json!({ "query": "tokio", "max_results": 0 }),
            json!({ "query": "tokio", "max_results": MAX_RESULTS + 1 }),
            json!({ "query": " " }),
        ] {
            let refused = component.execute(SEARCH_WEB, &params, None, &mut turn).await;
            assert!(matches!(refused, Err(ToolError::InvalidParameter(_))), "{refused:?}");
        }
        assert!(asked.lock().unwrap().is_empty(), "nothing asked");
        // The key is resolved of the secrets of whoever acts in the turn: here, there are none.
        let params = json!({ "query": "tokio" });
        let unresolved = component.execute(SEARCH_WEB, &params, None, &mut turn).await;
        assert!(
            matches!(&unresolved, Err(ToolError::ExecutionFailed(m)) if m.contains("TAVILY_KEY")),
            "{unresolved:?}"
        );
        assert!(asked.lock().unwrap().is_empty(), "nothing asked without the key");
    }
}
