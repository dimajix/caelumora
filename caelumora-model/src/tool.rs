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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    #[serde(default)]
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub function_name: String,
    pub arguments: Value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: Option<String>,
    pub tool_name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
    pub tool_call: ToolCall,
    pub description: String,
    pub result: Option<String>
}