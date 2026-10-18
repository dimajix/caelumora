//! Session templates (issue 106), as a client offers them for a new session.

use crate::scope::ScopeKind;
use caelumora_model::template::{ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::value::language::Language;
use caelumora_model::value::time_zone::TimeZone;
use serde::Serialize;

/// A session template, as a client offers it for a new session - its id,
/// name and description, not the template itself (issue 106).
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: SessionTemplateId,
    pub name: String,
    /// Empty if it says nothing.
    pub description: String,
    /// The language its sessions are held in unless another is asked for
    /// (issue 425); `null` for none in particular.
    pub language: Option<Language>,
    /// The time zone its sessions are held in unless another is asked for
    /// (issue 424); `null` for none - a client then offers its user's.
    pub time_zone: Option<TimeZone>,
    /// The workspace its sessions have unless another is chosen (issue
    /// 266); absent if it names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<TemplateWorkspaceResponse>,
}

/// The workspace a template names: found by its name where it lies, or
/// made there - in a session's scope, made for every session anew.
#[derive(Debug, Serialize)]
pub struct TemplateWorkspaceResponse {
    pub name: String,
    pub scope: ScopeKind,
    pub storage: String,
}

impl From<&SessionTemplate> for TemplateResponse {
    fn from(template: &SessionTemplate) -> Self {
        // One of a group's scope is left out: no session is made from it.
        let workspace = template.workspace.as_ref().and_then(|w| {
            let scope = match w.scope {
                ScopeTemplate::SessionScope => ScopeKind::Session,
                ScopeTemplate::UserScope => ScopeKind::User,
                ScopeTemplate::GlobalScope => ScopeKind::Global,
                ScopeTemplate::GroupScope => return None,
            };
            Some(TemplateWorkspaceResponse { name: w.name.to_string(), scope, storage: w.storage.clone() })
        });
        Self { id: template.id, name: template.name.to_string(), description: template.description.as_ref().map(ToString::to_string).unwrap_or_default(), language: template.language.clone(), time_zone: template.time_zone, workspace }
    }
}
