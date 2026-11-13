//! The tools on the attachments an agent heard (issues 282, 533): the component kind `attachments`
//! (`component`), whose tools reach them by the numbers its transcript gave them, and the library
//! that joins it to the kernel (`library`). The numbers are the transcript's, the attachments the
//! kernel's: this is built on the agents' tools and the views of the API, as the tools of any other
//! subject are.

mod component;
pub mod library;
#[cfg(test)]
mod testing;

/// The namespace of the texts of the tools in the prompt catalogue (issue 57).
pub(crate) const NAMESPACE: &str = "tool_attachment";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
pub(crate) const PROMPTS: &str = include_str!("prompts.yaml");
