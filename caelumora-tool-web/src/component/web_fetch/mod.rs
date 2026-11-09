//! The component kind `web_fetch` (issue 544): what it is configured with and built of - the
//! client that fetches its pages (`client`), from what addresses (`address`), read as text
//! (`text`) -; its tool `fetch_page` is its facet `ToolProvider` (`tool`).

mod address;
mod client;
mod text;
mod tool;

use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, ApprovalMode, KeepWholeAbove, ToolProvider};
use caelumora_api::component::{ComponentError, ComponentFacets, ComponentKind};
use caelumora_api::component::{LibraryComponents, Params};
use caelumora_api::error::describe_request_error;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::tool::Tool;
use client::Client;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
const WEB_FETCH_KIND: &str = "web_fetch";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebFetchParams {
    /// Whether calls require approval. Left out, they do not: a fetch only reads, what leaves is
    /// its URL, which goes only where everything is public, and what it reaches is bounded by the
    /// rules of the client.
    #[serde(default = "ApprovalMode::never")]
    requires_approval: ApprovalMode,
    /// What the tool is for, as its tool set tells it; left out, the catalogue's text.
    #[serde(default)]
    description: Option<String>,
    /// From how many bytes of its text on a page is kept whole, and a preview given in its stead;
    /// left out, no page is (issue 547).
    #[serde(default)]
    keep_whole_above_bytes: Option<KeepWholeAbove>,
}

impl Params for WebFetchParams {}

/// A component of the kind `web_fetch`: the tool `fetch_page`, which fetches a page of the web by
/// its URL.
#[derive(Debug)]
struct WebFetchComponent {
    client: Client,
    description: String,
    tool: Tool,
    /// The texts its results give.
    prompts: PromptTexts,
    /// From how many bytes of its text on a page is kept whole, if any is.
    keep_whole_above: Option<KeepWholeAbove>,
}

impl WebFetchComponent {
    /// The component configured by `params`, fetching with `client`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(
        params: WebFetchParams,
        client: Client,
        prompts: PromptTexts,
    ) -> Result<Self, PromptError> {
        let tool = tool::fetch_page(&prompts, params.requires_approval.required(true))?;
        let description = match params.description {
            Some(description) => description,
            None => tool_text(&prompts, NAMESPACE, WEB_FETCH_KIND, "description", ())?,
        };
        let keep_whole_above = params.keep_whole_above_bytes;
        Ok(Self { client, description, tool, prompts, keep_whole_above })
    }
}

/// Registers the component kind `web_fetch`, which writes for the agent's model.
pub(super) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(WEB_FETCH_KIND)
                .params::<WebFetchParams>()
                .provides::<dyn ToolProvider>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let params = WebFetchParams::read(&descriptor.params)?;
            let client = Client::new().map_err(|e| {
                ComponentError::InvalidConfig(format!(
                    "no HTTP client for '{}': {}",
                    descriptor.id,
                    describe_request_error(e)
                ))
            })?;
            let component = WebFetchComponent::new(params, client, prompts)?;
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
    use caelumora_domain::confidentiality::Classification;
    use caelumora_kernel::component::testing::{built, descriptor};
    use caelumora_kernel::testing::TestKernel;
    use serde_json::json;

    /// Built of nothing but the library: its tool asks for no approval unless configured, and its
    /// URLs go where everything is public.
    #[tokio::test]
    async fn a_component_is_built_with_its_tool_and_cleared_for_public_alone() {
        let kernel =
            TestKernel { libraries: vec![&AgentsLibrary, &WebToolLibrary], ..Default::default() }
                .build();
        let fetch = descriptor("fetch", WEB_FETCH_KIND, json!({}));
        let provider = built::<dyn ToolProvider>(&kernel, fetch).await.unwrap();
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "fetch_page");
        assert!(!tools[0].requires_approval, "no approval unless configured");
        assert_eq!(provider.destination(), Some(Classification::PUBLIC));
        assert_eq!(provider.description(), "Fetch pages of the web by their URL.");

        let asking = descriptor("asking", WEB_FETCH_KIND, json!({ "requires_approval": true }));
        let asking = built::<dyn ToolProvider>(&kernel, asking).await.unwrap();
        assert!(asking.tools()[0].requires_approval, "as configured");
        assert!(
            WebFetchParams::read(&json!({ "destination": { "level": "internal" } })).is_err(),
            "its destination is not configured: what it fetches from is anyone"
        );
        let below = json!({ "keep_whole_above_bytes": 1 });
        assert!(WebFetchParams::read(&below).is_err(), "a threshold below the preview");
    }
}
