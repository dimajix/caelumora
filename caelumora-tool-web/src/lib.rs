//! The tools that reach the web (issue 544): the component kind `web_fetch` (`component`), whose
//! tool fetches a page by its URL and gives it its model as text, and the library that joins it to
//! the kernel (`library`). A crate of its own, so that the kernel takes on no parser of HTML.

mod component;
pub mod library;

#[cfg(test)]
use caelumora_agents::testing;
#[cfg(test)]
use caelumora_domain::prompt_catalogue::PromptTexts;

/// The namespace of the texts of the tools in the prompt catalogue (issue 57).
pub(crate) const NAMESPACE: &str = "tool_web";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
pub(crate) const PROMPTS: &str = include_str!("prompts.yaml");

/// The texts of the kernel, of the agents and of these tools, as the kernel and the libraries add
/// them - for what a test makes itself.
#[cfg(test)]
pub(crate) fn prompts() -> PromptTexts {
    testing::prompts_with(NAMESPACE, PROMPTS)
}
