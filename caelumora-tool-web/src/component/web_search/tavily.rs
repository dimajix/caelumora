//! A search with Tavily (issue 544): its API's request - the query, how many results, no answer
//! of its own and no content of the pages beyond a snippet - and its answer, read as what was
//! found (`Found`).

use super::{Found, SearchError};
use caelumora_api::error::describe_request_error;
use caelumora_domain::remote::RemoteEndpoint;
use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::{json, Value};

/// How many characters of what Tavily says of a failure a message gives.
const MAX_EXPLANATION_CHARS: usize = 200;

/// How many characters of a page's snippet a result gives.
const MAX_SNIPPET_CHARS: usize = 1000;

/// Tavily's answer, as far as it is read.
#[derive(Deserialize)]
struct Answer {
    results: Vec<Hit>,
}

/// One result of Tavily's answer; a field that is missing or `null` is read as none.
#[derive(Deserialize)]
struct Hit {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

/// What Tavily at `endpoint`, asked with `key`, finds for `query`: at most `max_results` pages,
/// those without a URL left out, each snippet cut to `MAX_SNIPPET_CHARS`.
///
/// # Errors
/// `Failed` for an endpoint that is no URL or was not reached, `Status` where it refused,
/// `InvalidAnswer` for an answer that is not Tavily's.
pub(super) async fn search(
    http: &HttpClient,
    endpoint: &RemoteEndpoint,
    key: &str,
    query: &str,
    max_results: usize,
) -> Result<Vec<Found>, SearchError> {
    let failed = |reason: String| SearchError::Failed { endpoint: endpoint.shown(), reason };
    let url =
        endpoint.url_of(&["search"]).ok_or_else(|| failed("the endpoint is no URL".into()))?;
    let request = http.post(url).bearer_auth(key).json(&json!({
        "query": query,
        "max_results": max_results,
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
        "include_images": false,
    }));
    let response = request.send().await.map_err(|e| failed(describe_request_error(e)))?;
    let status = response.status();
    if !status.is_success() {
        let body: Value = response.json().await.unwrap_or_default();
        return Err(SearchError::Status {
            endpoint: endpoint.shown(),
            status: status.as_u16(),
            explanation: explanation(&body),
        });
    }
    let answer: Answer = response.json().await.map_err(|e| SearchError::InvalidAnswer {
        endpoint: endpoint.shown(),
        reason: describe_request_error(e),
    })?;
    Ok(answer
        .results
        .into_iter()
        .filter_map(|hit| {
            Some(Found {
                url: hit.url?,
                title: hit.title.unwrap_or_default(),
                snippet: hit.content.unwrap_or_default().chars().take(MAX_SNIPPET_CHARS).collect(),
            })
        })
        .collect())
}

/// What Tavily says of a failure in `body` - `{"detail": {"error": "..."}}` -, after a colon and
/// bounded; nothing where it says nothing.
fn explanation(body: &Value) -> String {
    let said = body["detail"]["error"].as_str().or_else(|| body["detail"].as_str());
    match said {
        Some(said) => format!(": {}", said.chars().take(MAX_EXPLANATION_CHARS).collect::<String>()),
        None => String::new(),
    }
}
