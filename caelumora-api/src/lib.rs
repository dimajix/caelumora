//! The kernel's API (issue 516): what a library is given and declares - the interface of the agent
//! side -, and nothing that decides. It depends on the model alone; the kernel implements its
//! traits, and a library links it and the model, never the kernel.

pub mod actor;
pub mod command;
pub mod component;
pub mod error;
pub mod library;
pub mod logging;
pub mod model;
pub mod reader;
pub mod remote;
pub mod runtime;
pub mod terminal;
pub mod transport;
pub mod workspace;
