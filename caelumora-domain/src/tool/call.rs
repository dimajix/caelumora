//! A call of a tool as it is recorded (`ToolCall`), how it ended (`ToolCallResult`,
//! `ToolResultStatus`) with what its tool gave its model (`ToolContent`), and where it stands while
//! its turn runs (`ToolCallState`, `ToolCallStatus`).

use super::{ToolPresentation, ToolVisibility};
use crate::actor::ActorEvent;
use crate::approval::{ApprovalDecision, ApprovalId, ToolMatcher};
use crate::confidentiality::Classification;
use crate::content::ContentRef;
use crate::context::KnowledgeRef;
use crate::question::{Answers, QuestionId, Questions};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub function_name: String,
    /// `Null` where the call was published without its arguments.
    pub arguments: Value,
    /// The labels of the tool at the time of the call, stamped by the agent:
    /// what applied then counts, even if the tool changes later.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    #[serde(default, skip_serializing_if = "ToolPresentation::is_default")]
    pub presentation: ToolPresentation,
    /// What its tool says the call does, in words for a person - put to whoever decides on it,
    /// and shown with it -, stamped by the agent as it admits the call (issue 586). `None` where
    /// the tool says nothing of a call beyond its arguments, where it was answered before it was
    /// described, and where the arguments it is made from are withheld (`withhold_arguments`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ToolCall {
    /// A call of `function_name` with `arguments`, under `id` if it has one: shown and presented
    /// as by default until the agent stamps it with what its tool declares.
    pub fn new(id: Option<String>, function_name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id,
            function_name: function_name.into(),
            arguments,
            visibility: ToolVisibility::default(),
            presentation: ToolPresentation::default(),
            description: None,
        }
    }

    /// Withholds its arguments, and the description made of them, where they are not to be shown.
    pub fn withhold_arguments(&mut self) {
        self.arguments = Value::Null;
        self.description = None;
    }
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
    /// The `AccessPolicy` does not permit the call.
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
    /// 148), or the server restarted or the session was stopped before it
    /// ran (issues 75, 377).
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

    /// The status as the content of a result the tool did not give, with the `message` that says
    /// what happened and what to do next where there is one: `{"status": …, "message": …}`, else
    /// `{"status": …}`.
    pub fn as_content(self, message: Option<&str>) -> String {
        let mut content = serde_json::json!({ "status": self });
        if let Some(message) = message {
            content["message"] = message.into();
        }
        content.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: Option<String>,
    pub tool_name: String,
    /// What the model is given of the result in words (issue 586), beside its tool and its call's
    /// id and what it gave and showed (`files`, `shown`): the tool's output, or for any other
    /// status a JSON object of the `status` and, where it can be had, a `message`; the status
    /// alone where only how the call ended is given (`ToolResultStatus::as_content`,
    /// `Message::names_only`), and empty where the result is not published
    /// (`Message::published`).
    pub content: String,
    /// Omitted when `Ok`.
    #[serde(default, skip_serializing_if = "ToolResultStatus::is_ok")]
    pub status: ToolResultStatus,
    /// Who sees it: the call's flags; `content` is empty where it was
    /// published without its result.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    /// How the result is presented (issue 586): its call's, stamped by `of` as `visibility` is.
    #[serde(default, skip_serializing_if = "ToolPresentation::is_default")]
    pub presentation: ToolPresentation,
    /// How sensitive `content` is and for whom (issue 133): as the tool
    /// classified its result, else as the tool is configured, else as the
    /// channel. Never above the channel: what may not flow into it is
    /// withheld.
    pub classification: Classification,
    /// When the tool started and finished running - or was interrupted
    /// (issue 161) - only for a call that ran, not for one a decision or a
    /// check answered. Stored with the
    /// history; published with the call, even where its result is
    /// withheld: when it ran is the call's, not the result's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// The questions the call asked and how they were answered or declined
    /// (issue 165), for the record - kept with the history and the
    /// transcript, published as `content` is, never given to the model,
    /// which learns of them only what the tool put into `content`. `None`
    /// for a call that asked nothing, or sensitive questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Answers>,
    /// Who approved or denied the call, if it needed approval (issue 419):
    /// whose answers decided it, each with the rule that answered for them,
    /// if one did, and when; empty where it timed out or nobody may approve.
    /// Whether it was approved says `status`. For the record, as `answers`,
    /// and never given to the model; published with the call even where
    /// its result is withheld, since every user was told the decision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval: Vec<ApprovalDecision>,
    /// What the tool gave its model besides `content` (issue 282): seen, not said - no attachment
    /// of the message, heard by no other agent, published as `content` is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ContentRef>,
    /// The attachments the call showed its model again, heard before - their references, not their
    /// content kept again (issue 282); published as `content` is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<ContentRef>,
    /// What the tool read or wrote for the result its model was given (issue 538) - a file of a
    /// workspace by its place, the history of a channel of a session -, so that the transcript
    /// holds what the agent knows, as a `TranscriptEntry::Given` does; recorded with the turn as
    /// well. Never given to the model, nor published (`Message::published`): who may read what the
    /// agent saw reads it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<KnowledgeRef>,
}

impl ToolCallResult {
    /// How `call` ended: `status`, and `content` for the model, classified
    /// `classification`. Carries the call's id, tool, flags and presentation, so that the
    /// result is published by what applied to the call, and shown as it was.
    pub fn of(
        call: &ToolCall,
        status: ToolResultStatus,
        content: String,
        classification: Classification,
    ) -> Self {
        Self {
            id: call.id.clone(),
            tool_name: call.function_name.clone(),
            content,
            status,
            visibility: call.visibility,
            presentation: call.presentation,
            classification,
            started_at: None,
            finished_at: None,
            answers: None,
            approval: Vec::new(),
            files: Vec::new(),
            shown: Vec::new(),
            knowledge: Vec::new(),
        }
    }
}

/// What a tool gives its model besides its value (issues 512, 547), before it is kept as the
/// result's (`ToolCallResult::files`): a file by its name and its bytes, kept as they are, whatever
/// they are (issue 552). What it is, its bytes tell, never the name nor the tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolContent {
    pub name: String,
    pub bytes: Bytes,
}

/// A tool call of the current step of a turn, and where it stands (issue
/// 165): kept with the turn, so that a step resumes after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub tool_call: ToolCall,
    pub status: ToolCallStatus,
    /// Who decided its approval, once it was decided (issue 419): kept from
    /// the decision on, whatever the call waits for next, and taken into its
    /// result as it ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval: Vec<ApprovalDecision>,
}

/// Where a tool call stands in its life - not how it ended, which its
/// result says (`ToolResultStatus`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "kept once per call of a step; boxing the result would only cost an allocation"
)]
pub enum ToolCallStatus {
    /// A person decides whether it may run (issue 13), shown the call and its description - a
    /// decision of their own if `explicit`, which no rule of a person answers - and offered a rule
    /// for the calls `suggested` matches, if one can be derived from the call (issue 400). `id` is
    /// the approval's, from the agent's request to the decision (issue 81). Approved, it asks its
    /// `questions`, if it has any (issue 165).
    WaitingApproval {
        id: ApprovalId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        explicit: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggested: Option<ToolMatcher>,
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
    pub fn pending(
        tool_call: ToolCall,
        explicit: bool,
        suggested: Option<ToolMatcher>,
        questions: Option<Questions>,
    ) -> Self {
        Self {
            tool_call,
            status: ToolCallStatus::WaitingApproval {
                id: ApprovalId::new(),
                explicit,
                suggested,
                questions,
            },
            approval: Vec::new(),
        }
    }

    /// `tool_call`, waiting for the answers to `questions` (`ToolCallStatus::WaitingUser`).
    pub fn asking(tool_call: ToolCall, questions: Questions) -> Self {
        Self {
            tool_call,
            status: ToolCallStatus::WaitingUser { id: QuestionId::new(), questions },
            approval: Vec::new(),
        }
    }

    /// `tool_call`, to be run as it is.
    pub fn approved(tool_call: ToolCall) -> Self {
        Self { tool_call, status: ToolCallStatus::Ready { answers: None }, approval: Vec::new() }
    }

    /// `tool_call`, answered with `result` before it ran: it is not run.
    pub fn answered(tool_call: ToolCall, result: ToolCallResult) -> Self {
        Self { tool_call, status: ToolCallStatus::Finished(result), approval: Vec::new() }
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

    /// The event that withdraws the request it waits for, if it waits for a
    /// person: its approval's, or its questions, closed for those they were
    /// shown to.
    pub fn withdrawal(&self) -> Option<ActorEvent> {
        match &self.status {
            ToolCallStatus::WaitingApproval { id, .. } => {
                Some(ActorEvent::ApprovalRequestWithdrawn { id: *id })
            }
            ToolCallStatus::WaitingUser { id, questions } => {
                Some(ActorEvent::QuestionClosed { id: *id, shown_to: questions.shown_to() })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::testing::{make_tool_call, make_tool_description};

    #[test]
    fn a_call_state_is_read_back_as_written() {
        let call = ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            description: Some("Delete it?".to_string()),
            ..make_tool_call("delete")
        };
        let waiting = ToolCallState::pending(
            call.clone(),
            true,
            Some(ToolMatcher::proposed(
                &make_tool_description(&call.function_name),
                &call.arguments,
            )),
            None,
        );
        let read: ToolCallState =
            serde_json::from_str(&serde_json::to_string(&waiting).unwrap()).unwrap();
        assert_eq!(read.approval_id(), waiting.approval_id());
        assert!(matches!(read.status, ToolCallStatus::WaitingApproval { explicit: true, .. }));
        assert_eq!(read.tool_call.description.as_deref(), Some("Delete it?"), "with its call");
        let ready: ToolCallState = serde_json::from_str(
            &serde_json::to_string(&ToolCallState::approved(call.clone())).unwrap(),
        )
        .unwrap();
        assert!(matches!(ready.status, ToolCallStatus::Ready { answers: None }));
    }

    /// A status is written as the content of a result the tool did not give, with its message
    /// where there is one.
    #[test]
    fn a_status_is_written_as_a_result_s_content() {
        let denied = ToolResultStatus::Denied;
        assert_eq!(
            denied.as_content(Some("declined")),
            r#"{"message":"declined","status":"denied"}"#
        );
        assert_eq!(denied.as_content(None), r#"{"status":"denied"}"#);
    }

    /// Issue 586: a result is presented as its call, written with the presentation unless it is
    /// `default`.
    #[test]
    fn a_result_is_presented_as_its_call() {
        let ended = |call: &ToolCall| {
            ToolCallResult::of(call, ToolResultStatus::Ok, "{}".to_string(), Default::default())
        };
        let shell = ToolPresentation::new("shell");
        let result = ended(&ToolCall { presentation: shell, ..make_tool_call("run_command") });
        assert_eq!(result.presentation, shell);
        assert_eq!(serde_json::to_value(&result).unwrap()["presentation"], "shell");
        let plain = serde_json::to_value(ended(&make_tool_call("look"))).unwrap();
        assert!(plain.get("presentation").is_none(), "`default` is not written");
        let read: ToolCallResult = serde_json::from_value(plain).unwrap();
        assert_eq!(read.presentation, ToolPresentation::DEFAULT);
    }
}
