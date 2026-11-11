//! `GroupSpec`: what a new team should be, as it was asked.

use caelumora_domain::scope::Scope;

/// What a new team should be, as it was asked: validated as `GroupManager::create` makes it.
#[derive(Debug, Clone)]
pub struct GroupSpec {
    pub name: String,
    /// The global scope or its owner's own.
    pub scope: Scope,
}

impl GroupSpec {
    /// A team of `name` in `scope`.
    pub fn new(name: impl Into<String>, scope: Scope) -> Self {
        Self { name: name.into(), scope }
    }
}
