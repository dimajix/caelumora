use crate::approval::{ApprovalId, ToolMatcher};
use crate::question::{Answers, QuestionId, Questions};
use crate::confidentiality::{Classification, Visibility};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolParameters {
    Object {
        properties: HashMap<String, ToolParameters>,
        // Left out when empty, so it must be read as empty when missing.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// Who sees a tool's calls (issues 54, 133): the call itself - that the
/// tool was called - and its content, the arguments and the result. The
/// content is never seen by more than the call. The agent keeps every call
/// in full; what it publishes to its channel is redacted by these flags
/// (`Message::published`). How sensitive a result is, is another question:
/// its classification (`ToolCallResult::classification`, issue 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVisibility {
    pub call: Visibility,
    pub content: Visibility,
}

impl ToolVisibility {
    /// Call, arguments and result are published to the channel.
    pub const SHOWN: ToolVisibility = ToolVisibility { call: Visibility::Channel, content: Visibility::Channel };
    /// Others may see that the tool was called, but not with what or what
    /// it returned.
    pub const CALL_ONLY: ToolVisibility = ToolVisibility { call: Visibility::Channel, content: Visibility::Owner };
    /// Nothing of the call leaves the agent.
    pub const HIDDEN: ToolVisibility = ToolVisibility { call: Visibility::Owner, content: Visibility::Owner };

    pub fn is_shown(&self) -> bool {
        *self == Self::SHOWN
    }
}

impl Default for ToolVisibility {
    fn default() -> Self {
        Self::SHOWN
    }
}

/// How a frontend shows a tool's calls and their results (issue 165), so
/// that it need not recognise a tool by its name, which components and
/// configuration choose. Display only: what a call does, and whether it
/// waits for anyone, does not depend on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPresentation {
    /// As every call: its name, its arguments and its result.
    #[default]
    Default,
    /// A shell command (issue 251): its `command` as a line of code, its
    /// `output` as a terminal shows it, and its exit code.
    Shell,
    /// A plan (issue 263): its `summary` and its `steps`, numbered.
    Plan,
    /// A file read (issue 251): its `path`, and the `content` of its result
    /// as text, with the lines it covers - `start_line`, `end_line`,
    /// `total_lines` - and where to read on (`next`).
    FileRead,
    /// A file written: its `path` and its `content` as text, and how many
    /// `bytes` its result says were written.
    FileWrite,
    /// A replacement in a file: its `path`, its `old_text` and `new_text`
    /// as text, whether it replaces every occurrence (`replace_all`), and
    /// how many `replacements` its result counts.
    FileEdit,
    /// A directory listed (issue 265): its `path`, and the `entries` of its
    /// result - each `name`, `is_dir`, `size` and `modified` - and whether
    /// they are `complete`.
    FileList,
    /// Files searched (issue 265): its `pattern` and `path`, and the
    /// `matches` of its result - each `path`, `line` and `text` - with how
    /// many files were searched and skipped, and whether it is `complete`.
    FileSearch,
    /// A file or directory moved or renamed (issue 267): its `from` and
    /// `to`, and whether its result says it was `renamed` - moved within
    /// its directory.
    FileMove,
    /// Questions put to users (issue 268): its `questions` as they were
    /// put, and whom they wait for (`ask`); once answered, nothing beyond
    /// the record of the answers every call that asked shows.
    Questions,
    /// A file of the workspace offered for download (issue 273): its
    /// `name`, `size`, `description` and `path`, and whether the file is
    /// still as it was (`size`, `modified`) or gone.
    FilePublish,
    /// A file published as a version of an artifact (issue 271): its
    /// `name`, `version`, `size`, `description` and the `artifact` it is
    /// downloaded from.
    ArtifactPublish,
}

impl ToolPresentation {
    pub fn is_default(&self) -> bool {
        *self == Self::Default
    }
}

/// A tool of an agent: what its model is told of it - its name, what it
/// does, its parameters - and what only the runtime acts on, whether its
/// calls need approval, who sees them, how its results are classified. A
/// backend sends the model the first three alone; the record of what a
/// model was offered keeps all of it, as it was then (issues 137, 194).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    /// Left out in a record made before issue 194, which kept only what the
    /// model was told: not known, read as no.
    #[serde(default)]
    pub requires_approval: bool,
    /// Who sees its calls. Approvers see the arguments of a call that needs
    /// approval regardless: they cannot decide otherwise.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    /// How its results are classified where they do not classify
    /// themselves (issue 133) - configured with the tool, e.g. for an MCP
    /// server. `None`: as the channel the call is made in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// How frontends show its calls.
    #[serde(default, skip_serializing_if = "ToolPresentation::is_default")]
    pub presentation: ToolPresentation,
}

/// The tools of one source of an agent - a component, e.g. an MCP server -
/// with what they are for (issue 194).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSet {
    /// The id of the component the tools come from, unique within the
    /// agent.
    pub name: String,
    /// What the tools are for, written for a model.
    pub description: String,
    pub tools: Vec<Tool>,
    /// Why the source gives no tools now, in general terms - never the
    /// error, which may name hosts (issue 32). `None`: available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Where the arguments of its calls go (issue 180): the classification
    /// its remote side is cleared for. `None`: they stay in the system, or
    /// are bounded otherwise - a workspace's storage by its own ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<Classification>,
}

impl ToolSet {
    /// The available set `name` of `tools`, for `description`.
    pub fn new(name: impl Into<String>, description: impl Into<String>, tools: Vec<Tool>) -> Self {
        Self { name: name.into(), description: description.into(), tools, unavailable: None, destination: None }
    }

    /// It as it is offered in a channel its remote side is not cleared for
    /// (issue 180): unavailable, without its tools.
    pub fn withheld(self) -> Self {
        Self { tools: Vec::new(), unavailable: Some("its remote side is not cleared for this channel".to_string()), ..self }
    }
}

/// The tool sets a model was offered as they were recorded: as sets since
/// issue 194, before as a flat list of tools - read as one set without a
/// name or description.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum RecordedToolSets {
    Sets(Vec<ToolSet>),
    Flat(Vec<Tool>),
}

impl From<RecordedToolSets> for Vec<ToolSet> {
    fn from(recorded: RecordedToolSets) -> Self {
        match recorded {
            RecordedToolSets::Sets(sets) => sets,
            RecordedToolSets::Flat(tools) => vec![ToolSet::new("", "", tools)],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub function_name: String,
    /// `Null` where the call was published without its content.
    pub arguments: Value,
    /// The labels of the tool at the time of the call, stamped by the agent:
    /// what applied then counts, even if the tool changes later.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    #[serde(default, skip_serializing_if = "ToolPresentation::is_default")]
    pub presentation: ToolPresentation,
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
    /// A person declined to answer the questions of the call (issue 165):
    /// the tool did not run.
    Declined,
    /// The tool rejected the arguments.
    InvalidArguments,
    /// The tool ran and failed.
    Failed,
    /// The tool never ran: the turn that made the call ended first (issue
    /// 148), or the server restarted before it ran (issue 75).
    NotExecuted,
    /// The tool ran, but was interrupted before it ended: the turn that made
    /// the call was cancelled or stopped meanwhile (issue 161). What it did
    /// before is done; whether it took effect is unknown.
    Interrupted,
    /// No tool of that name exists.
    UnknownTool,
    /// The tool ran, but its result is classified so that it may not flow
    /// into the channel (issue 133); the model is told so instead.
    Withheld,
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
    /// Who sees it: the call's flags; `content` is empty where it was
    /// published without its content.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    /// How sensitive `content` is and for whom (issue 133): as the tool
    /// classified its result, else as the tool is configured, else as the
    /// channel. Never above the channel: what may not flow into it is
    /// withheld.
    pub classification: Classification,
    /// When the tool started and finished running - or was interrupted
    /// (issue 161) - only for a call that ran, not for one a decision or a
    /// check answered. Stored with the
    /// history; published with the call, even where its content is
    /// withheld: when it ran is the call's, not the content's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The questions the call asked and how they were answered or declined
    /// (issue 165), for the record - kept with the history and the
    /// transcript, published as `content` is, never given to the model,
    /// which learns of them only what the tool put into `content`. `None`
    /// for a call that asked nothing, or sensitive questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Answers>,
}

impl ToolCallResult {
    /// How `call` ended: `status`, and `content` for the model, classified
    /// `classification`. Carries the call's id, tool and flags, so that the
    /// result is published by what applied to the call.
    pub fn of(call: &ToolCall, status: ToolResultStatus, content: String, classification: Classification) -> Self {
        Self {
            id: call.id.clone(),
            tool_name: call.function_name.clone(),
            content,
            status,
            visibility: call.visibility,
            classification,
            started_at: None,
            finished_at: None,
            answers: None,
        }
    }

    /// The result as a model is given it: without the record of the
    /// answers to the call's questions, which is not the model's.
    pub fn for_the_model(&self) -> ToolCallResult {
        ToolCallResult { answers: None, ..self.clone() }
    }
}

/// A tool call of the current step of a turn, and where it stands (issue
/// 165): kept with the turn, so that a step resumes after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub tool_call: ToolCall,
    pub status: ToolCallStatus,
}

/// Where a tool call stands in its life - not how it ended, which its
/// result says (`ToolResultStatus`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallStatus {
    /// A person decides whether it may run (issue 13). They are shown
    /// `description` - a decision of their own if `explicit`, which no rule
    /// of a person answers - and offered a rule for the calls `suggested`
    /// matches. `id` is the approval's, from the agent's request to the
    /// decision (issue 81). Approved, it asks its `questions`, if it has
    /// any (issue 165).
    WaitingApproval {
        id: ApprovalId,
        description: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        explicit: bool,
        suggested: ToolMatcher,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        questions: Option<Questions>,
    },
    /// A person gives the answers it runs with (issue 165); `id` is the
    /// questions', from the agent's request to the answer.
    WaitingUser { id: QuestionId, questions: Questions },
    /// It may run, with the `answers` to its questions if it asked any.
    /// They are never stored: a call ready when its turn is resumed after a
    /// restart is not run, but answered as not executed.
    Ready {
        #[serde(skip)]
        answers: Option<Answers>,
    },
    /// It ended - ran, or was answered before it ran - with its one result
    /// (issue 100).
    Finished(ToolCallResult),
}

impl ToolCallState {
    /// `tool_call`, waiting for a person's decision (`ToolCallStatus::WaitingApproval`),
    /// and then to ask `questions`.
    pub fn pending(tool_call: ToolCall, description: String, explicit: bool, suggested: ToolMatcher, questions: Option<Questions>) -> Self {
        Self { tool_call, status: ToolCallStatus::WaitingApproval { id: ApprovalId::new(), description, explicit, suggested, questions } }
    }

    /// `tool_call`, waiting for the answers to `questions` (`ToolCallStatus::WaitingUser`).
    pub fn asking(tool_call: ToolCall, questions: Questions) -> Self {
        Self { tool_call, status: ToolCallStatus::WaitingUser { id: QuestionId::new(), questions } }
    }

    /// `tool_call`, to be run as it is.
    pub fn approved(tool_call: ToolCall) -> Self {
        Self { tool_call, status: ToolCallStatus::Ready { answers: None } }
    }

    /// `tool_call`, answered with `result` before it ran: it is not run.
    pub fn answered(tool_call: ToolCall, result: ToolCallResult) -> Self {
        Self { tool_call, status: ToolCallStatus::Finished(result) }
    }

    /// Its result, once it ended.
    pub fn result(&self) -> Option<&ToolCallResult> {
        match &self.status {
            ToolCallStatus::Finished(result) => Some(result),
            _ => None,
        }
    }

    /// Whether it ended.
    pub fn is_finished(&self) -> bool {
        self.result().is_some()
    }

    /// The approval it waits for, if it does.
    pub fn approval_id(&self) -> Option<ApprovalId> {
        match &self.status {
            ToolCallStatus::WaitingApproval { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The questions it waits for the answers to, if it does.
    pub fn question_id(&self) -> Option<QuestionId> {
        match &self.status {
            ToolCallStatus::WaitingUser { id, .. } => Some(*id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_without_required_ones_are_read_back_as_written() {
        let parameters = ToolParameters::Object { properties: HashMap::from([("channel".to_string(), ToolParameters::string("a channel"))]), required: Vec::new() };
        let json = serde_json::to_string(&parameters).unwrap();
        assert!(!json.contains("required"), "an empty list is left out");
        assert_eq!(serde_json::from_str::<ToolParameters>(&json).unwrap(), parameters);
    }

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

    #[test]
    fn a_call_state_is_read_back_as_written() {
        let call = ToolCall { id: Some("1".to_string()), function_name: "delete".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let waiting = ToolCallState::pending(call.clone(), "Delete it?".to_string(), true, ToolMatcher::exact(&call), None);
        let read: ToolCallState = serde_json::from_str(&serde_json::to_string(&waiting).unwrap()).unwrap();
        assert_eq!(read.approval_id(), waiting.approval_id());
        assert!(matches!(read.status, ToolCallStatus::WaitingApproval { explicit: true, ref description, .. } if description == "Delete it?"));
        let ready: ToolCallState = serde_json::from_str(&serde_json::to_string(&ToolCallState::approved(call.clone())).unwrap()).unwrap();
        assert!(matches!(ready.status, ToolCallStatus::Ready { answers: None }));
    }
}