use crate::approval::{ApprovalId, ApprovalStatus};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolId(pub Uuid);

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

/// What participants other than the calling agent see of a call of a tool
/// (issue 54). The agent itself always has the full call and result in its
/// context; only what it publishes to the channel is redacted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolVisibility {
    /// The call, its arguments and its result.
    #[default]
    Public,
    /// Only that the tool was called - no arguments, no result.
    CallOnly,
    /// Nothing at all.
    Hidden,
}

impl ToolVisibility {
    pub fn is_public(&self) -> bool {
        *self == ToolVisibility::Public
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    #[serde(default)]
    pub requires_approval: bool,
    /// What others see of a call. Approvers see the arguments of a call that
    /// needs approval regardless: they cannot decide otherwise.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_public")]
    pub visibility: ToolVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub function_name: String,
    /// `Null` where the call was published with `ToolVisibility::CallOnly`.
    pub arguments: Value,
    /// The visibility of the tool at the time of the call, stamped by the
    /// agent: what applied then counts, even if the tool changes later.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_public")]
    pub visibility: ToolVisibility,
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
    /// Omitted when `Ok`, so results stored before it existed read as such.
    #[serde(default, skip_serializing_if = "ToolResultStatus::is_ok")]
    pub status: ToolResultStatus,
    /// The visibility of the call this is the result of; `content` is empty
    /// where it was published with `ToolVisibility::CallOnly`.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_public")]
    pub visibility: ToolVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
    pub tool_call: ToolCall,
    pub description: String,
    pub result: Option<String>,
    /// How the call ended, once `result` is set.
    #[serde(default)]
    pub result_status: ToolResultStatus,
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