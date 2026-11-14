//! The words in which both ways of drawing tell what is printed, each styling them its own way
//! (issue 569).

use crate::printed::ErrorKind;
use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::content::ContentRef;
use caelumora_domain::question::{Question, QuestionRequest};
use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
use serde_json::Value;

/// How long a tool's arguments, or the first line of its result, are told at most, in characters.
const SHORTENED: usize = 160;

/// What tells that a rule asks for a call to be decided by a person.
pub(super) const EXPLICIT: &str =
    "A rule of this session or channel asks for this call to be decided by a person.";

/// What a tool is called with, as the terminal tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Arguments {
    /// Nothing.
    None,
    /// What is not shown in the channel.
    NotShown,
    /// Its arguments in one line, shortened.
    Shown(String),
}

impl Arguments {
    /// What `call` is called with.
    pub(super) fn of(call: &ToolCall) -> Self {
        match &call.arguments {
            Value::Null => Arguments::NotShown,
            Value::Object(arguments) if arguments.is_empty() => Arguments::None,
            arguments => Arguments::Shown(shortened(&arguments.to_string())),
        }
    }
}

/// How the call of `result` ended, in words that follow the tool's name: what it answered,
/// shortened to its first line - or that it is not shown -, or why it did not.
pub(super) fn outcome(result: &ToolCallResult) -> String {
    let not_shown = !result.visibility.result.is_channel() && result.content.is_empty();
    if not_shown && result.status.is_ok() {
        return "answered, its result not shown".to_string();
    }
    let lines = || result.content.lines().filter(|line| !line.trim().is_empty());
    let first = lines().next().map(shortened);
    let more = lines().count().saturating_sub(1);
    let said = |how: &str| match &first {
        Some(first) if more > 0 => format!("{how}: {first} (and {more} lines more)"),
        Some(first) => format!("{how}: {first}"),
        None => how.to_string(),
    };
    match result.status {
        ToolResultStatus::Ok => said("answered"),
        ToolResultStatus::Failed => said("failed"),
        ToolResultStatus::InvalidArguments => said("refused its arguments"),
        ToolResultStatus::Denied => "was denied".to_string(),
        ToolResultStatus::Forbidden => "is forbidden here".to_string(),
        ToolResultStatus::TimedOut => "was not approved in time".to_string(),
        ToolResultStatus::Declined => "did not run: its questions were declined".to_string(),
        ToolResultStatus::NotExecuted => "did not run".to_string(),
        ToolResultStatus::Interrupted => "was interrupted".to_string(),
        ToolResultStatus::UnknownTool => "is no tool".to_string(),
        ToolResultStatus::Withheld => "answered what may not be shown here".to_string(),
    }
}

/// The word an error of `kind` is told by.
pub(super) fn error_word(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Failed => "Error",
        ErrorKind::Rejected => "Rejected",
        ErrorKind::Refused => "Refused",
    }
}

/// What tells of `attachment`: its name, type and size.
pub(super) fn attachment(attachment: &ContentRef) -> String {
    let name = attachment.name.as_str();
    format!("{name}, {}, {}", attachment.media_type, attachment.size_shown())
}

/// The line that asks `question`, the one of `index` of its request: its number, header and
/// question.
pub(crate) fn question_line(index: usize, question: &Question) -> String {
    format!("{}. {}: {}", index + 1, question.header, question.question)
}

/// What tells that so many `events` of a channel were missed.
pub(super) fn missed(events: u64) -> String {
    format!("{events} events of this channel missed.")
}

/// What tells how to answer the approval `request`.
pub(super) fn how_to_approve(request: &ApprovalRequest) -> String {
    let id = request.id.0;
    format!("Type '/approve {id}' or '/deny {id}' to respond.")
}

/// The rows that ask the questions of `request`, each with its choices below it.
pub(super) fn question_rows(request: &QuestionRequest) -> Vec<String> {
    let mut rows = Vec::new();
    for (index, question) in request.questions.questions.iter().enumerate() {
        rows.push(question_line(index, question));
        for (number, choice) in question.choices.iter().enumerate() {
            let recommended = if choice.recommended { " (recommended)" } else { "" };
            let description =
                choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
            rows.push(format!("   {}) {}{recommended}{description}", number + 1, choice.label));
        }
    }
    rows
}

/// What tells how to answer the questions of `request` if they are `put_to_user` - with a
/// warning where they are sensitive -, else that they are put to others.
pub(super) fn how_to_answer(request: &QuestionRequest, put_to_user: bool) -> Vec<String> {
    let id = request.id.0;
    if !put_to_user {
        return vec!["They are put to others.".to_string()];
    }
    let mut how = Vec::new();
    if request.questions.sensitive {
        how.push(
            "Sensitive: the answers go to the tool alone - but what you type is shown on this \
             terminal."
                .to_string(),
        );
    }
    how.push(format!(
        "Type '/answer {id}' to answer them one by one, or '/decline {id}'. A message instead \
         declines them."
    ));
    how
}

/// `text` shortened to at most `SHORTENED` characters, the cut marked.
fn shortened(text: &str) -> String {
    match text.char_indices().nth(SHORTENED) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}
