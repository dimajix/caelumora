//! The kernel's API (issue 516): what a library is given and declares - the interface of the agent
//! side -, and nothing that decides. Its parts name nothing else of the kernel, which implements
//! its traits, so that they can stand in a crate of their own (`caelumora-api`, step 6 of issue
//! 516).

pub mod actor;
pub mod command;
pub mod component;
pub mod error;
pub mod library;
pub mod logging;
pub mod reader;
pub mod remote;
pub mod runtime;
pub mod terminal;
pub mod transport;
pub mod workspace;
