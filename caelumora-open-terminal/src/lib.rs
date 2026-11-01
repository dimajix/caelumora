//! Storages of workspaces backed by Open Terminal (issues 251, 327): `open_terminal_shared`, one
//! instance started outside Caelumora, and `open_terminal_docker`, a container per workspace
//! started over the Docker daemon - behind the feature `docker`. Both are backends of the kernel's
//! `TerminalProvider`, registered by the plugin (`plugin`); a crate of its own, so that the kernel
//! names no sandbox and the kernel is built without `bollard`.
//!
//! How an instance is reached, and what it does differently from the
//! `local` provider, is in `terminal`.

pub mod plugin;

mod client;
#[cfg(feature = "docker")]
mod docker;
mod shared;
mod terminal;
mod wire;

#[cfg(test)]
mod stand_in;
