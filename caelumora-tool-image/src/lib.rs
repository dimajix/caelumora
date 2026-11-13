//! The tool that generates images (issues 512, 525): the component kind `image_generation`
//! (`component`), whose tool generates an image with one of the image models its agent is given,
//! and the library that joins it to the kernel (`library`). The image models are the platform's, of
//! the plugin `caelumora-model`: the tool names no backend of them.

mod component;
pub mod library;
#[cfg(test)]
mod testing;

/// The namespace of the texts of the tool in the prompt catalogue (issue 57).
pub(crate) const NAMESPACE: &str = "tool_image";

/// Their texts, added to the catalogue under `NAMESPACE` by the library.
pub(crate) const PROMPTS: &str = include_str!("prompts.yaml");
