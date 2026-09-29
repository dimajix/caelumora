use crate::approval::{ApprovalId, ApprovalStatus};
use crate::confidentiality::Confidentiality;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolParameters {
    Object {
        properties: HashMap<String, ToolParameters>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
    },
    String {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Array {
        items: Box<ToolParameters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Boolean {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl ToolParameters {
    pub fn string(description: impl Into<String>) -> Self {
        Self::String {
            description: Some(description.into()),
        }
    }

    pub fn number(description: impl Into<String>) -> Self {
        Self::Number {
            description: Some(description.into()),
        }
    }

    pub fn boolean(description: impl Into<String>) -> Self {
        Self::Boolean {
            description: Some(description.into()),
        }
    }

    pub fn array(items: ToolParameters, description: impl Into<String>) -> Self {
        Self::Array {
            items: Box::new(items),
            description: Some(description.into()),
        }
    }

    pub fn object(
        properties: impl IntoIterator<Item = (impl Into<String>, ToolParameters)>,
        required: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::Object {
            properties: properties
                .into_iter()
                .map(|(k, v)| (k.into(), v))
                .collect(),
            required: required.into_iter().map(|s| s.into()).collect(),
        }
    }
}

/// The confidentiality labels of a tool's calls (issues 54, 61): how far
/// the call itself - that the tool was called - may travel, and how far its
/// content, the arguments and the result. The content is never less
/// confidential than the call. The agent keeps every call in full; what it
/// publishes to its channel is redacted by these labels against the
/// channel's level (`Message::published`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfidentiality {
    pub call: Confidentiality,
    pub content: Confidentiality,
}

impl ToolConfidentiality {
    /// Call, arguments and result may travel anywhere.
    pub const PUBLIC: ToolConfidentiality = ToolConfidentiality { call: Confidentiality::Public, content: Confidentiality::Public };
    /// Others may see that the tool was called, but not with what or what
    /// it returned.
    pub const CALL_ONLY: ToolConfidentiality = ToolConfidentiality { call: Confidentiality::Public, content: Confidentiality::Private };
    /// Nothing of the call leaves the agent.
    pub const HIDDEN: ToolConfidentiality = ToolConfidentiality { call: Confidentiality::Private, content: Confidentiality::Private };

    pub fn is_public(&self) -> bool {
        *self == Self::PUBLIC
    }
}

impl Default for ToolConfidentiality {
    fn default() -> Self {
        Self::PUBLIC
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    pub requires_approval: bool,
    /// How far its calls may travel. Approvers see the arguments of a call
    /// that needs approval regardless: they cannot decide otherwise.
    #[serde(default, skip_serializing_if = "ToolConfidentiality::is_public")]
    pub confidentiality: ToolConfidentiality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub function_name: String,
    /// `Null` where the call was published without its content.
    pub arguments: Value,
    /// The labels of the tool at the time of the call, stamped by the agent:
    /// what applied then counts, even if the tool changes later.
    #[serde(default, skip_serializing_if = "ToolConfidentiality::is_public")]
    pub confidentiality: ToolConfidentiality,
}

/// How a tool call ended (issue 15). Everything but `Ok` means the tool did
/// not produce a result, and the model is told why in a form it can act on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    /// The tool ran; `content` is its result.
    #[default]
    Ok,
    /// A user declined the call.
    Denied,
    /// The `SecurityManager` does not permit the call.
    Forbidden,
    /// No approval arrived in time.
    TimedOut,
    /// The tool rejected the arguments.
    InvalidArguments,
    /// The tool ran and failed.
    Failed,
    /// No tool of that name exists.
    UnknownTool,
}

impl ToolResultStatus {
    pub fn is_ok(&self) -> bool {
        *self == ToolResultStatus::Ok
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: Option<String>,
    pub tool_name: String,
    pub content: String,
    /// Omitted when `Ok`.
    #[serde(default, skip_serializing_if = "ToolResultStatus::is_ok")]
    pub status: ToolResultStatus,
    /// The labels of the call this is the result of; `content` is empty
    /// where it was published without its content.
    #[serde(default, skip_serializing_if = "ToolConfidentiality::is_public")]
    pub confidentiality: ToolConfidentiality,
    /// When the tool started and finished running - only for a call that
    /// ran, not for one a decision or a check answered. Stored with the
    /// history; published with the call, even where its content is
    /// withheld: when it ran is the call's, not the content's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ToolCallResult {
    /// How `call` ended: `status`, and `content` for the model. Carries the
    /// call's id, tool and labels, so that the result is published by what
    /// applied to the call.
    pub fn of(call: &ToolCall, status: ToolResultStatus, content: String) -> Self {
        Self {
            id: call.id.clone(),
            tool_name: call.function_name.clone(),
            content,
            status,
            confidentiality: call.confidentiality,
            started_at: None,
            finished_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
    pub tool_call: ToolCall,
    pub description: String,
    /// How the call ended - its one result, from the executor or from a
    /// decision that settled it (issue 100) - once it has.
    pub result: Option<ToolCallResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolean_and_array_round_trip_through_json() {
        let params = ToolParameters::object(
            vec![
                ("enabled", ToolParameters::boolean("Whether the feature is enabled")),
                ("tags", ToolParameters::array(ToolParameters::string("A tag"), "List of tags")),
            ],
            vec!["enabled"],
        );

        let json = serde_json::to_value(&params).unwrap();
        let round_tripped: ToolParameters = serde_json::from_value(json).unwrap();
        assert_eq!(params, round_tripped);

        match round_tripped {
            ToolParameters::Object { properties, .. } => {
                assert!(matches!(properties.get("enabled"), Some(ToolParameters::Boolean { .. })));
                assert!(matches!(properties.get("tags"), Some(ToolParameters::Array { .. })));
            }
            _ => panic!("Expected Object"),
        }
    }
}