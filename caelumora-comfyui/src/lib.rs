//! ComfyUI (issue 512): the workflows of a ComfyUI server the operator configured (`workflow`), and
//! the plugin that reads them (`plugin`). A crate of its own, so that a program that reaches no
//! ComfyUI server need not build it.

pub mod plugin;
pub mod workflow;
