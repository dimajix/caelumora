//! The facet `ToolProvider` of `web_fetch` (issue 544): the tool `fetch_page`, which fetches a
//! page of the web by its URL and gives it as text, a page at a time (`page_of`, issue 282), public
//! as anything anyone can fetch. A rule derived from a call is about the pages of its host, not the
//! one page (`ToolProvider::matcher`). A page above the component's threshold is kept whole beside
//! a preview of it (issue 547).

use super::client::{FetchError, Refusal};
use super::{WebFetchComponent, WEB_FETCH_KIND};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::tool::{
    arguments, described, page_of, page_parameters, proposed_matcher, TextPosition, ToolError,
    ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_domain::approval::{ArgumentMatcher, ToolMatcher};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::remote::redact_endpoint;
use caelumora_domain::tool::{Tool, ToolContent, ToolParameters, ToolPresentation, ToolVisibility};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The one tool of the component.
const FETCH_PAGE: &str = "fetch_page";

/// A page fetched: its `url`, and of its result the `url` it was found at and its `title`, the
/// `content` as text with the lines it covers - `start_line`, `start_offset`, `end_line`,
/// `total_lines` - and where to read on (`next`).
const WEB_PAGE_PRESENTATION: ToolPresentation = ToolPresentation::new("web_page");

/// How many bytes of a page `fetch_page` gives at once, as `read_file` gives 64 KiB unless
/// configured.
const MAX_PAGE_BYTES: usize = 64 * 1024;

/// The arguments of a call of `fetch_page`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fetch {
    url: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    start_offset: Option<usize>,
    #[serde(default)]
    line_count: Option<usize>,
}

/// The tool `fetch_page`, its texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of its texts cannot be had.
pub(super) fn fetch_page(
    prompts: &PromptTexts,
    requires_approval: bool,
) -> Result<Tool, PromptError> {
    let tool = Tool {
        name: FETCH_PAGE.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [("url", ToolParameters::string())].into_iter().chain(page_parameters()),
            ["url"],
        ),
        requires_approval,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: WEB_PAGE_PRESENTATION,
    };
    described(prompts, NAMESPACE, WEB_FETCH_KIND, tool, ())
}

/// What the model is told of a page not fetched: a URL that is none, or that the rules do not
/// follow, is its to correct; what the server or the way to it did is the call's failure.
impl From<FetchError> for ToolError {
    fn from(error: FetchError) -> Self {
        match error {
            FetchError::InvalidUrl { .. }
            | FetchError::Refused(
                Refusal::Scheme(_) | Refusal::InsideNetwork(_) | Refusal::Proxy(_),
            ) => ToolError::InvalidParameter(error.to_string()),
            _ => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

/// The matcher of the calls that fetch a page of the host of `url`, by any scheme of the web, on
/// any port, in any case: what a rule derived from a call of `fetch_page` is about. None for what
/// is no URL, or has no host. A host written in Unicode is matched in the form `Url` gives it,
/// punycode, so that the rule does not match the call it was derived from, and a person is asked
/// again.
fn same_host(url: &str) -> Option<ToolMatcher> {
    let url = Url::parse(url).ok()?;
    let host = regex::escape(url.host_str()?);
    // What follows the host begins a port, a path, a query or a fragment - never credentials,
    // behind which another host would stand (`https://docs.rs:x@elsewhere`).
    let pattern = format!("(?i)https?://{host}(:[0-9]+)?([/?#].*)?");
    Some(ToolMatcher {
        tool: FETCH_PAGE.to_string(),
        arguments: BTreeMap::from([("url".to_string(), ArgumentMatcher::Pattern(pattern))]),
    })
}

#[async_trait]
impl ToolProvider for WebFetchComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![self.tool.clone()]
    }

    /// Public: what a URL carries goes to anyone.
    fn destination(&self) -> Option<Classification> {
        Some(Classification::PUBLIC)
    }

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        _turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        if name != FETCH_PAGE {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let Fetch { url, start_line, start_offset, line_count } = arguments(params)?;
        let page = self.client.fetch(&url).await?;
        let whole_asked = start_line.is_none() && start_offset.is_none() && line_count.is_none();
        let kept = whole_asked && self.keep_whole_above.is_some_and(|a| a.keeps(&page.text));
        let mut output = if kept {
            let name = format!("{}.txt", page.url.host_str().unwrap_or("page"));
            let files = vec![ToolContent { name, bytes: page.text.clone().into() }];
            ToolOutput::kept(Some(&page.text), files, &self.prompts)
        } else {
            let shown = format!("'{}'", redact_endpoint(page.url.as_str()));
            let start = TextPosition::asked(start_line, start_offset);
            page_of(&page.text, &shown, start, line_count, MAX_PAGE_BYTES)?
                .value(&self.prompts)
                .into()
        };
        output.value["url"] = json!(page.url.as_str());
        if let Some(title) = page.title {
            output.value["title"] = json!(title);
        }
        Ok(ToolOutput { classification: Some(Classification::PUBLIC), ..output })
    }

    /// The calls that fetch a page of the same host; as the parameters tell for any other.
    fn matcher(&self, name: &str, params: &Value) -> ToolMatcher {
        let url = params.get("url").and_then(Value::as_str);
        match url.filter(|_| name == FETCH_PAGE).and_then(same_host) {
            Some(matcher) => matcher,
            None => proposed_matcher(&self.tools(), name, params),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::Client;
    use super::super::WebFetchParams;
    use super::*;
    use crate::prompts;
    use axum::routing::get;
    use axum::Router;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_agents::testing::{
        calling, detached_actor_runtime, register_scripted, saying, ONCE_MODEL_KIND,
    };
    use caelumora_agents::tool::{ApprovalMode, KeepWholeAbove};
    use caelumora_api::component::{ComponentFacets, ComponentKind};
    use caelumora_domain::actor::ActorEvent;
    use caelumora_domain::agent::{AgentChannelState, AgentId, Personality};
    use caelumora_domain::approval::Coverage;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::tool::{ToolCall, ToolResultStatus};
    use caelumora_domain::work::Work;
    use caelumora_kernel::testing::World;
    use caelumora_tool_attachment::library::AttachmentToolLibrary;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time;

    /// The component, fetching from the host itself too, keeping no page.
    fn component() -> WebFetchComponent {
        built(None)
    }

    /// The component, keeping a page whole above a part: `/long` is, `/page` is not.
    fn keeping() -> WebFetchComponent {
        built(Some(KeepWholeAbove::try_from(MAX_PAGE_BYTES).unwrap()))
    }

    fn built(keep_whole_above_bytes: Option<KeepWholeAbove>) -> WebFetchComponent {
        let params = WebFetchParams {
            requires_approval: ApprovalMode::never(),
            description: None,
            keep_whole_above_bytes,
        };
        WebFetchComponent::new(params, Client::reaching_the_host(), prompts()).unwrap()
    }

    /// What `component` gives for a call of `fetch_page` with `params`, in a turn that reaches no
    /// channel.
    async fn fetched(component: &WebFetchComponent, params: Value) -> ToolResult<ToolOutput> {
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(
            Default::default(),
            None,
            Default::default(),
            Default::default(),
        ));
        let mut turn = Turn::of(&mut state, detached_actor_runtime(AgentId::new())).unwrap();
        component.execute(FETCH_PAGE, &params, None, &mut turn).await
    }

    /// A server of the test on the host itself, with a page of three paragraphs, a text of one
    /// line longer than a part, and such a text that begins with a control character; its port.
    async fn server() -> u16 {
        let page = "<title>Three</title><p>one</p><p>two</p><p>three</p>";
        let long = "x".repeat(MAX_PAGE_BYTES + 1);
        let controls = format!("\u{1}{}", "x".repeat(MAX_PAGE_BYTES));
        let router = Router::new()
            .route("/page", get(move || async move { ([("content-type", "text/html")], page) }))
            .route("/long", get(move || async move { ([("content-type", "text/plain")], long) }))
            .route(
                "/controls",
                get(move || async move { ([("content-type", "text/plain")], controls) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    #[tokio::test]
    async fn a_page_is_given_a_part_at_a_time_with_where_it_was_found_and_public() {
        let port = server().await;
        let url = format!("http://localhost:{port}/page");
        let component = component();
        assert_eq!(component.tools()[0].presentation, WEB_PAGE_PRESENTATION, "shown as a page");
        let whole = fetched(&component, json!({ "url": url })).await.unwrap();
        assert_eq!(whole.classification, Some(Classification::PUBLIC));
        assert_eq!(whole.value["url"], json!(url));
        assert_eq!(whole.value["title"], json!("Three"));
        assert_eq!(whole.value["content"], json!("one\n\ntwo\n\nthree"));
        let part =
            fetched(&component, json!({ "url": url, "start_line": 3, "line_count": 1 })).await;
        let part = part.unwrap();
        assert_eq!(part.value["content"], json!("two\n"));
        assert_eq!(part.value["total_lines"], json!(5), "says how much there is: {}", part.value);
    }

    #[tokio::test]
    async fn a_line_longer_than_a_part_is_given_in_parts_of_it() {
        let port = server().await;
        let url = format!("http://localhost:{port}/long");
        let component = component();
        let first = fetched(&component, json!({ "url": url })).await.unwrap().value;
        assert_eq!(first["content"].as_str().map(str::len), Some(MAX_PAGE_BYTES));
        assert!(first["next"].as_str().unwrap().contains("start_offset 65536"), "{first}");
        let rest = json!({ "url": url, "start_line": 1, "start_offset": MAX_PAGE_BYTES });
        let last = fetched(&component, rest).await.unwrap().value;
        assert_eq!(last["content"], json!("x"));
        assert_eq!(last["start_offset"], json!(MAX_PAGE_BYTES));
        assert!(last.get("next").is_none(), "{last}");
    }

    #[tokio::test]
    async fn a_page_above_the_threshold_is_kept_whole_beside_a_preview() {
        let port = server().await;
        let url = format!("http://localhost:{port}/long");
        let component = keeping();
        let output = fetched(&component, json!({ "url": url })).await.unwrap();
        let [kept] = output.files.as_slice() else { panic!("one file: {:?}", output.files) };
        assert_eq!(kept.name, "localhost.txt");
        assert_eq!(kept.bytes.len(), MAX_PAGE_BYTES + 1, "the whole page");
        let preview = output.value["content"].as_str().unwrap();
        assert!(preview.len() < MAX_PAGE_BYTES, "{}", preview.len());
        let next = output.value["next"].as_str().unwrap();
        assert!(next.contains("attachment") && !next.contains("start_"), "{next}");
        assert_eq!(output.value["url"], json!(url));
        assert_eq!(output.classification, Some(Classification::PUBLIC));
    }

    #[tokio::test]
    async fn a_page_is_kept_only_where_it_is_asked_whole_and_above_a_threshold() {
        let port = server().await;
        let long = format!("http://localhost:{port}/long");
        let none = fetched(&component(), json!({ "url": long })).await.unwrap();
        assert!(none.files.is_empty(), "no threshold, nothing kept");
        let keeping = keeping();
        let page = format!("http://localhost:{port}/page");
        let short = fetched(&keeping, json!({ "url": page })).await.unwrap();
        assert!(short.files.is_empty(), "at most the threshold");
        let asked = [("start_line", 1), ("start_offset", 0), ("line_count", 1)];
        for (parameter, value) in asked {
            let params = json!({ "url": long, parameter: value });
            let part = fetched(&keeping, params.clone()).await.unwrap();
            assert_eq!(part.value["content"].as_str().map(str::len), Some(MAX_PAGE_BYTES));
            assert!(part.files.is_empty(), "a part asked for: {params}");
        }
        let controls = format!("http://localhost:{port}/controls");
        let output = fetched(&keeping, json!({ "url": controls })).await.unwrap();
        let [kept] = output.files.as_slice() else { panic!("one file: {:?}", output.files) };
        assert_eq!(kept.bytes[0], 1, "a control character is kept as any other");
    }

    /// A page kept is offered and found in the next step of the very turn that fetched it, though
    /// the transcript numbered no attachment as the turn began.
    #[tokio::test]
    async fn a_page_kept_is_read_in_the_next_step_of_the_turn_that_fetched_it() {
        let port = server().await;
        let url = format!("http://localhost:{port}/long");
        let answers = vec![
            calling("fetch_page", json!({ "url": url })),
            calling("read_attachment", json!({ "ref": "a1", "start_offset": MAX_PAGE_BYTES })),
            saying("read"),
        ];
        let world = World::with(&[], &[&AgentsLibrary, &AttachmentToolLibrary], |registry| {
            register_scripted(registry, ONCE_MODEL_KIND, answers);
            registry.register(
                ComponentKind::new("fetching").provides::<dyn ToolProvider>(),
                |_, _| {
                    let mut facets = ComponentFacets::empty();
                    facets.insert::<dyn ToolProvider>(Arc::new(keeping()));
                    Ok(facets)
                },
            );
        })
        .await;
        let mut working = Personality {
            components: ComponentConfig {
                component_nodes: vec![
                    ComponentDescriptor::new("model", ONCE_MODEL_KIND),
                    ComponentDescriptor::new("fetching", "fetching"),
                    ComponentDescriptor::new("attachments", "attachments"),
                ],
            },
            ..world.personality("working", None, &[]).await
        };
        let personalities = &world.kernel.entity_stores().personalities;
        personalities.update(&World::system(), &mut working).await.unwrap();
        let helper = world.agent("helper", world.alice, &working).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let alice = World::as_user(world.alice);
        main.reclassify(&alice, Classification::PUBLIC).await.expect("where the tool is offered");
        let mut watch = main.user_actor(world.alice).expect("she takes part").watch();
        main.user_actor(world.alice)
            .unwrap()
            .send_message(Some("@helper read it".to_string()), None, Vec::new())
            .await
            .expect("sent");

        let read = time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::CompleteMessageGenerated(message) =
                    watch.recv().await.expect("the channel runs").event
                {
                    if let Some(result) =
                        message.tool_results.iter().find(|r| r.tool_name == "read_attachment")
                    {
                        break (result.status, result.content.clone());
                    }
                }
            }
        })
        .await
        .expect("the attachment is read");
        assert_eq!(read.0, ToolResultStatus::Ok, "{}", read.1);
        assert!(read.1.contains(r#""content":"x""#), "the end of the page kept: {}", read.1);
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn what_cannot_be_fetched_is_told_as_the_models_to_correct_or_as_failed() {
        let component = component();
        let invalid = fetched(&component, json!({ "url": "ftp://example.org/" })).await;
        assert!(matches!(invalid, Err(ToolError::InvalidParameter(_))), "{invalid:?}");
        let inside = fetched(&component, json!({ "url": "http://10.0.0.1/" })).await;
        assert!(
            matches!(&inside, Err(ToolError::InvalidParameter(m))
                if m.contains("inside the network")),
            "{inside:?}"
        );
        let unreachable = fetched(&component, json!({ "url": "http://127.0.0.1:1/" })).await;
        assert!(matches!(unreachable, Err(ToolError::ExecutionFailed(_))), "{unreachable:?}");
    }

    /// A rule derived from a call is about the pages of its host, by http or https, on any port -
    /// and about no other host, also behind credentials.
    #[test]
    fn a_rule_derived_from_a_call_is_about_the_pages_of_its_host() {
        let component = component();
        let matcher = component.matcher(FETCH_PAGE, &json!({ "url": "https://docs.rs/tokio" }));
        let call = |url: &str| ToolCall {
            id: None,
            function_name: FETCH_PAGE.to_string(),
            arguments: json!({ "url": url }),
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        };
        for url in ["https://docs.rs/serde", "http://docs.rs:8080/?q=1", "https://docs.rs"] {
            assert_eq!(matcher.matches(&call(url)).unwrap(), Coverage::Whole, "{url}");
        }
        for url in
            ["https://docs.rs.evil.org/", "https://docs.rs:x@evil.org/", "https://evil.org/docs.rs"]
        {
            let coverage = matcher.matches(&call(url)).unwrap();
            assert_eq!(coverage, Coverage::Nothing, "{url}");
        }
        let written = "HTTPS://Docs.RS/tokio";
        let matcher = component.matcher(FETCH_PAGE, &json!({ "url": written }));
        assert_eq!(matcher.matches(&call(written)).unwrap(), Coverage::Whole, "in any case");
        assert_eq!(
            component.matcher(FETCH_PAGE, &json!({ "url": "no url" })),
            proposed_matcher(&component.tools(), FETCH_PAGE, &json!({ "url": "no url" })),
            "as the parameters tell where there is no host"
        );
    }
}
