//! The tools of a component discovered of its remote side (issues 126, 252): `DiscoveredTools` over
//! a `ToolSource` - an MCP server, an OpenAPI service -, which plugs in how its tools are
//! discovered and made, while `DiscoveredTools` keeps what was discovered for a time,
//! discovers again in the name of whoever acts, keeps the tools discovered before when that fails,
//! and describes itself as configured or as the source says of itself.

use super::{Tool, ToolProvider};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::cache::ExpiringCache;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::secret::Secrets;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::warn;

/// A remote side whose tools are discovered - an MCP server, an OpenAPI service -: what its
/// protocol plugs into a `DiscoveredTools`.
#[async_trait]
pub trait ToolSource: Debug + Send + Sync + 'static {
    /// What is kept of a discovery, from which the tools are made.
    type Discovery: Debug + Send + Sync + 'static;

    /// Its tools and what it says of itself, discovered anew in the name `secrets` are bound to.
    ///
    /// # Errors
    /// `Undiscovered` with why, in general terms and in full.
    async fn discover(&self, secrets: &dyn Secrets) -> Result<Self::Discovery, Undiscovered>;

    /// What was last discovered, kept for a time - which a tool of it may find outdated
    /// (`ExpiringCache::invalidate`).
    fn discovered(&self) -> &ExpiringCache<Self::Discovery>;

    /// What `discovery` says of the source itself, for the description of its tool set.
    fn described(discovery: &Self::Discovery) -> Option<&str>;

    /// The tools of `discovery`, each reaching this source.
    fn tools(self: &Arc<Self>, discovery: &Self::Discovery) -> Vec<Arc<dyn Tool>>;
}

/// Why the tools of a source could not be discovered.
#[derive(Debug)]
pub struct Undiscovered {
    /// Why, in general terms, for the model and the user (issue 32) - never the error, which may
    /// name hosts.
    pub reason: String,
    /// The error in full, for the log.
    pub cause: String,
}

/// The tools of a component discovered of a `ToolSource`, as its `ToolProvider`: none before they
/// are first offered, discovered in the name of whoever acts where they are offered, and again once
/// what was discovered is outdated. A source that cannot be asked leaves the tools discovered
/// before (issue 126), or, without any, the tool set unavailable with why in general terms (issue
/// 32) - a warning logged either way - and is asked again the next time.
#[derive(Debug)]
pub struct DiscoveredTools<S: ToolSource> {
    /// The component's id, which the log names.
    component_id: String,
    source: Arc<S>,
    /// What the tool set is for as configured, before what the source says of itself.
    description: Option<String>,
    /// What the tool set is for when neither is.
    unnamed: String,
    /// Where the arguments of its tools' calls go (`ToolProvider::destination`).
    destination: Classification,
}

impl<S: ToolSource> DiscoveredTools<S> {
    /// The tool set of the component `component_id` over `source`, described as `description`
    /// says, else as the source says of itself, else as `unnamed`; cleared for `destination`.
    pub fn new(
        component_id: String,
        source: Arc<S>,
        description: Option<String>,
        unnamed: String,
        destination: Classification,
    ) -> Self {
        Self { component_id, source, description, unnamed, destination }
    }
}

#[async_trait]
impl<S: ToolSource> ToolProvider for DiscoveredTools<S> {
    fn description(&self) -> String {
        let discovered = self.source.discovered().current();
        self.description
            .as_deref()
            .or_else(|| discovered.as_deref().and_then(S::described))
            .unwrap_or(&self.unnamed)
            .to_string()
    }

    /// Its tools as last discovered; none before they were.
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let discovered = self.source.discovered().current();
        discovered.map(|discovery| self.source.tools(&discovery)).unwrap_or_default()
    }

    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Arc<dyn Tool>>, String> {
        let source = &self.source;
        let secrets = situation.secrets();
        let id = &self.component_id;
        match source.discovered().get(|| source.discover(&*secrets)).await {
            Ok(discovery) => Ok(source.tools(&discovery)),
            Err(undiscovered) => match source.discovered().current() {
                Some(discovery) => {
                    let cause = &undiscovered.cause;
                    warn!("Tool component '{id}' failed to discover its tools again ({cause}); keeping those discovered before");
                    Ok(source.tools(&discovery))
                }
                None => {
                    let cause = &undiscovered.cause;
                    warn!("Tool component '{id}' failed to discover its tools ({cause}); no tools available");
                    Err(undiscovered.reason)
                }
            },
        }
    }

    fn destination(&self) -> Option<Classification> {
        Some(self.destination.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{described_tools, offered_anywhere};
    use crate::tool::{ToolOutput, ToolResult};
    use crate::turn::Turn;
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::question::Answers;
    use caelumora_domain::tool::testing::make_tool_description;
    use caelumora_domain::tool::ToolDescription;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A discovery of tools by their names, and what the source said of itself.
    #[derive(Debug)]
    struct Found {
        names: Vec<&'static str>,
        described: Option<&'static str>,
    }

    /// A source that answers each discovery with the next of what it was given.
    #[derive(Debug)]
    struct Scripted {
        discoveries: Mutex<VecDeque<Result<Found, Undiscovered>>>,
        discovered: ExpiringCache<Found>,
    }

    /// A tool of a `Scripted` source, which answers nothing.
    #[derive(Debug)]
    struct Named(ToolDescription);

    #[async_trait]
    impl Tool for Named {
        fn description(&self) -> &ToolDescription {
            &self.0
        }

        async fn execute(
            &self,
            _params: &Value,
            _answers: Option<&Answers>,
            _turn: &mut Turn<'_>,
        ) -> ToolResult<ToolOutput> {
            Ok(Value::Null.into())
        }
    }

    #[async_trait]
    impl ToolSource for Scripted {
        type Discovery = Found;

        async fn discover(&self, _secrets: &dyn Secrets) -> Result<Found, Undiscovered> {
            self.discoveries.lock().unwrap().pop_front().expect("a discovery was scripted")
        }

        fn discovered(&self) -> &ExpiringCache<Found> {
            &self.discovered
        }

        fn described(discovery: &Found) -> Option<&str> {
            discovery.described
        }

        fn tools(self: &Arc<Self>, discovery: &Found) -> Vec<Arc<dyn Tool>> {
            let tool = |name| Arc::new(Named(make_tool_description(name))) as Arc<dyn Tool>;
            discovery.names.iter().copied().map(tool).collect()
        }
    }

    fn found(
        names: Vec<&'static str>,
        described: Option<&'static str>,
    ) -> Result<Found, Undiscovered> {
        Ok(Found { names, described })
    }

    fn undiscovered() -> Result<Found, Undiscovered> {
        let reason = "its server cannot be reached".to_string();
        Err(Undiscovered { reason, cause: "connection refused by 10.0.0.1".to_string() })
    }

    /// A source that discovers as `discoveries` say.
    fn scripted(discoveries: Vec<Result<Found, Undiscovered>>) -> Arc<Scripted> {
        let discovered = ExpiringCache::new(Duration::from_secs(60));
        Arc::new(Scripted { discoveries: Mutex::new(discoveries.into()), discovered })
    }

    /// The tool set of the component `tracker` over `source`, described as `description` says.
    fn provider(description: Option<&str>, source: &Arc<Scripted>) -> DiscoveredTools<Scripted> {
        DiscoveredTools::new(
            "tracker".to_string(),
            Arc::clone(source),
            description.map(str::to_string),
            "The tools of 'tracker'.".to_string(),
            Classification::PUBLIC,
        )
    }

    fn names(tools: &[ToolDescription]) -> Vec<&str> {
        tools.iter().map(|tool| tool.name.as_str()).collect()
    }

    #[tokio::test]
    async fn a_tool_set_is_described_as_configured_else_as_its_source_says_else_as_unnamed() {
        let unconfigured = provider(None, &scripted(vec![found(vec![], Some("Tracks issues."))]));
        assert_eq!(unconfigured.description(), "The tools of 'tracker'.", "nothing discovered yet");
        offered_anywhere(&unconfigured).await.unwrap();
        assert_eq!(unconfigured.description(), "Tracks issues.");
        let source = scripted(vec![found(vec![], Some("Tracks issues."))]);
        let configured = provider(Some("Our tracker."), &source);
        offered_anywhere(&configured).await.unwrap();
        assert_eq!(configured.description(), "Our tracker.");
        let silent = provider(None, &scripted(vec![found(vec![], None)]));
        offered_anywhere(&silent).await.unwrap();
        assert_eq!(silent.description(), "The tools of 'tracker'.", "the source said nothing");
    }

    #[tokio::test]
    async fn tools_are_discovered_where_they_are_offered_and_kept_while_discovering_again_fails() {
        let source =
            scripted(vec![found(vec!["a", "b"], None), undiscovered(), found(vec!["c"], None)]);
        let provider = provider(None, &source);
        assert!(provider.tools().is_empty(), "not discovered before they are offered");
        assert_eq!(names(&offered_anywhere(&provider).await.unwrap()), ["a", "b"]);
        assert_eq!(names(&described_tools(&provider)), ["a", "b"], "kept");
        source.discovered().invalidate();
        assert_eq!(names(&offered_anywhere(&provider).await.unwrap()), ["a", "b"], "kept");
        assert_eq!(names(&offered_anywhere(&provider).await.unwrap()), ["c"], "asked again");
    }

    #[tokio::test]
    async fn without_tools_discovered_a_failure_leaves_the_set_unavailable_with_why_in_general() {
        let provider = provider(None, &scripted(vec![undiscovered(), found(vec!["a"], None)]));
        let unavailable = offered_anywhere(&provider).await.unwrap_err();
        assert_eq!(unavailable, "its server cannot be reached", "never the cause");
        assert_eq!(names(&offered_anywhere(&provider).await.unwrap()), ["a"], "asked again");
    }

    #[test]
    fn a_tool_set_is_cleared_for_its_destination() {
        let internal = Classification::general(Confidentiality::Internal);
        let unnamed = "The tools of 'tracker'.".to_string();
        let source = scripted(vec![]);
        let provider =
            DiscoveredTools::new("tracker".to_string(), source, None, unnamed, internal.clone());
        assert_eq!(provider.destination(), Some(internal));
    }
}
