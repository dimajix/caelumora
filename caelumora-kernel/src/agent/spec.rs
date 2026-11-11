//! `AgentSpec`: what a new agent should be, as it was asked.

use caelumora_domain::agent::PersonalityId;
use caelumora_domain::scope::Scope;

/// What a new agent should be, as it was asked: validated as `AgentManager::create` makes it.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub name: String,
    /// Where it lies: anywhere but in a project's scope.
    pub scope: Scope,
    /// The personality it acts with.
    pub personality_id: PersonalityId,
}

impl AgentSpec {
    /// An agent of `name` in `scope`, with the personality `personality_id`.
    pub fn new(name: impl Into<String>, scope: Scope, personality_id: PersonalityId) -> Self {
        Self { name: name.into(), scope, personality_id }
    }
}
