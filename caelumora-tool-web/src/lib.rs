//! The tools that reach the web (issue 544): the component kinds (`component`) `web_fetch`, whose
//! tool fetches a page by its URL and gives it its model as text, and `web_search`, whose tool
//! searches the web with a provider, and the library that joins them to the kernel (`library`). A
//! crate of its own, so that the kernel takes on no parser of HTML.

mod component;
pub mod library;
#[cfg(test)]
mod testing;

/// The namespace of the texts of the tools in the prompt catalogue (issue 57).
pub(crate) const NAMESPACE: &str = "tool_web";

/// Who fetches a page or searches, as the servers are told.
pub(crate) const USER_AGENT: &str = concat!("Caelumora/", env!("CARGO_PKG_VERSION"));

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
pub(crate) const PROMPTS: &str = include_str!("prompts.yaml");
