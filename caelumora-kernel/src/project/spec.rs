//! `ProjectSpec`: what a new shared project should be, as it was asked.

use caelumora_domain::scope::Scope;

/// What a new shared project should be, as it was asked: validated as `ProjectManager::create`
/// makes it.
#[derive(Debug, Clone)]
pub struct ProjectSpec {
    pub name: String,
    /// The global scope or its owner's own.
    pub scope: Scope,
    /// What it is for; none if `None`.
    pub description: Option<String>,
}

impl ProjectSpec {
    /// A project of `name` in `scope`, without a description.
    pub fn new(name: impl Into<String>, scope: Scope) -> Self {
        Self { name: name.into(), scope, description: None }
    }
}
