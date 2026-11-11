//! `WorkspaceSpec`: what a new workspace should be, as it was asked.

use caelumora_domain::confidentiality::Classification;
use caelumora_domain::scope::Scope;

/// What a new workspace should be, as it was asked: validated as `WorkspaceManager::create` makes
/// it.
#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    pub name: String,
    /// Where it lies: a session's scope, its owner's or the global one.
    pub scope: Scope,
    /// The storage its files lie in, by its configured name.
    pub storage: String,
    /// What it holds; none if `None`.
    pub description: Option<String>,
    /// The classification of all its files; that of a channel that names none unless named
    /// (`Classification::CHANNEL_DEFAULT`), so that a workspace and such a channel flow both ways.
    pub classification: Classification,
}

impl WorkspaceSpec {
    /// A workspace of `name` in `scope`, in `storage`, without a description and classified as a
    /// channel that names no classification.
    pub fn new(name: impl Into<String>, scope: Scope, storage: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scope,
            storage: storage.into(),
            description: None,
            classification: Classification::CHANNEL_DEFAULT,
        }
    }
}
