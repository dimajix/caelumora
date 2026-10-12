//! Agents as their readers list them (issue 164).

use caelumora_model::snapshot;
use caelumora_model::agent::AgentId;
use serde::Serialize;

/// An agent the requesting user may read (issue 164), with whether they
/// may delete it.
#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub id: AgentId,
    pub name: String,
    pub can_delete: bool,
}

impl From<snapshot::AgentSnapshot> for AgentResponse {
    fn from(view: snapshot::AgentSnapshot) -> Self {
        Self { id: view.id, name: view.name, can_delete: view.can_delete }
    }
}
