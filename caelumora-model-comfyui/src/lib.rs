//! Image models served by ComfyUI (issues 512, 525): the backend `comfyui` of the image models of
//! `caelumora-model` (`model`) - one workflow the operator configured (`workflow`) on one ComfyUI
//! server, reached by a client of it (`client`) - and the plugin that registers it (`plugin`). A
//! crate of its own, so that neither the models' plugin nor a tool names an image model provider.

mod client;
mod model;
pub mod plugin;
mod workflow;
