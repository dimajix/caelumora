//! The kernel's API (issue 516): what a library is given and declares - the interface of the agent
//! side -, and nothing that decides. It depends on the domain alone; the kernel implements its
//! traits, and a library links it and the domain, never the kernel.

pub mod actor;
pub mod cache;
pub mod command;
pub mod component;
pub mod error;
pub mod image;
pub mod library;
pub mod logging;
pub mod model;
pub mod reader;
pub mod runtime;
pub mod staging;
pub mod terminal;
pub mod transport;
pub mod workspace;
