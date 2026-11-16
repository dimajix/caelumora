//! The words in which both ways of drawing tell what is printed and what is asked, each styling
//! them its own way (issues 569, 571).

use crate::asking::{Asking, Offered};
use crate::printed::ErrorKind;
use caelumora_domain::approval::{ApprovalRequest, ArgumentMatcher, Reach, ToolMatcher};
use caelumora_domain::content::ContentRef;
use caelumora_domain::question::{Choice, Question, QuestionRequest};
use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
use caelumora_domain::workspace::CommandStatus;
use caelumora_tool_workspace::component::command_state;
use serde_json::Value;
use uuid::Uuid;

/// How many hexadecimal digits of an id the terminal names it by (`short_id`).
const SHORT_ID: usize = 8;

/// How long a tool's arguments, or the first line of its result, are told at most, in characters.
const SHORTENED: usize = 160;

/// What tells that a rule asks for a call to be decided by a person.
pub(super) const EXPLICIT: &str =
    "A rule of this session or channel asks for this call to be decided by a person.";

/// How a call is told: by what its tool says it does, or else by what it is called with (issue
/// 586).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Called {
    /// What its tool says it does.
    Described(String),
    /// With nothing.
    Bare,
    /// With what is not shown in the channel.
    NotShown,
    /// With its arguments, as JSON.
    With(String),
}

impl Called {
    /// How `call` is told, whole.
    pub(super) fn of(call: &ToolCall) -> Self {
        if let Some(description) = &call.description {
            return Called::Described(description.clone());
        }
        match (&call.arguments, arguments(call)) {
            (Value::Null, _) => Called::NotShown,
            (_, None) => Called::Bare,
            (_, Some(arguments)) => Called::With(arguments.to_string()),
        }
    }

    /// The same on one line, shortened (`one_line`, `shortened`): as a call is headed.
    pub(super) fn on_one_line(self) -> Self {
        match self {
            Called::Described(description) => Called::Described(one_line(&description)),
            Called::With(arguments) => Called::With(shortened(&arguments)),
            whole => whole,
        }
    }
}

/// What `call` is called with, where it is called with anything that is shown.
pub(super) fn arguments(call: &ToolCall) -> Option<&Value> {
    match &call.arguments {
        Value::Null => None,
        Value::Object(arguments) if arguments.is_empty() => None,
        arguments => Some(arguments),
    }
}

/// `count` lines, in words.
fn lines_counted(count: usize) -> String {
    match count {
        1 => "1 line".to_string(),
        count => format!("{count} lines"),
    }
}

/// What whoever decides on `call` is told of it, whole: what its tool says it does, or else the
/// tool and its arguments.
pub(super) fn to_decide(call: &ToolCall) -> String {
    let tool = &call.function_name;
    match Called::of(call) {
        Called::Described(description) => description,
        Called::Bare => format!("call {tool}"),
        Called::NotShown => format!("call {tool}, its arguments not shown"),
        Called::With(arguments) => format!("call {tool} with {arguments}"),
    }
}

/// How the call of `result` ended, in words that follow the tool's name, and whether it failed -
/// the call, or the command it ran: of a command (`command_state`), where it stands and how much
/// it printed; else what it answered, shortened to its first line - or that it is not shown -, or
/// why it did not.
pub(super) fn outcome(result: &ToolCallResult) -> (String, bool) {
    if let Some(state) = command_state(result) {
        let stands = match (state.status, state.exit_code) {
            (CommandStatus::Running, _) => format!("still running as {}", state.id),
            (CommandStatus::Completed, Some(code)) => format!("exit code {code}"),
            (CommandStatus::Completed, None) => "completed".to_string(),
            (CommandStatus::Killed, _) => "killed".to_string(),
            (CommandStatus::Failed, _) => "failed".to_string(),
        };
        let printed = lines_counted(state.output.lines().count());
        let failed =
            state.status == CommandStatus::Failed || state.exit_code.is_some_and(|code| code != 0);
        return (format!("{stands}, {printed}"), failed);
    }
    (answered(result), !result.status.is_ok())
}

/// What the call of `result` answered, in words that follow the tool's name: shortened to its first
/// line - or that it is not shown -, or why it did not answer.
fn answered(result: &ToolCallResult) -> String {
    let not_shown = !result.visibility.result.is_channel() && result.content.is_empty();
    if not_shown && result.status.is_ok() {
        return "answered, its result not shown".to_string();
    }
    let lines = || result.content.lines().filter(|line| !line.trim().is_empty());
    let first = lines().next().map(shortened);
    let more = lines().count().saturating_sub(1);
    let said = |how: &str| match &first {
        Some(first) if more > 0 => format!("{how}: {first} (and {} more)", lines_counted(more)),
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
fn question_line(index: usize, question: &Question) -> String {
    format!("{}. {}: {}", index + 1, question.header, question.question)
}

/// The words of `choice`: its label, recommended and described where it is.
fn choice_words(choice: &Choice) -> String {
    let recommended = if choice.recommended { " (recommended)" } else { "" };
    let description = choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
    format!("{}{recommended}{description}", choice.label)
}

/// What is asked, in words: the call to be approved, or the next question with its number.
pub(super) fn what_is_asked(asking: &Asking) -> String {
    match asking {
        Asking::Approval(request) => {
            format!("Approve? {}", one_line(&to_decide(&request.tool_call)))
        }
        Asking::Question { .. } => {
            asking.question().map(|(index, q)| question_line(index, q)).unwrap_or_default()
        }
    }
}

/// The words of the choice `offered`: yes - from now on for the calls alike, within its reach -,
/// no, or the words of a question's choice.
pub(super) fn offered_words(offered: &Offered) -> String {
    match offered {
        Offered::Approve => "Yes".to_string(),
        Offered::ApproveFromNowOn { matcher, reach } => {
            let within = match reach {
                Reach::Channel(_) => "in this channel",
                Reach::Session(_) => "in this session",
                Reach::Everywhere => "everywhere",
            };
            format!("Yes, and from now on for {} {within}", calls_alike(matcher))
        }
        Offered::Deny => "No".to_string(),
        Offered::Choice(choice) => choice_words(choice),
    }
}

/// The calls `matcher` is about, in words: of its tool, with what its arguments are compared by.
fn calls_alike(matcher: &ToolMatcher) -> String {
    let tool = &matcher.tool;
    let arguments: Vec<String> = matcher
        .arguments
        .iter()
        .map(|(name, argument)| match argument {
            ArgumentMatcher::Shell { shell } => {
                format!("{name} beginning with `{}`", one_line(&shell.join(" ")))
            }
            ArgumentMatcher::Pattern(pattern) => format!("{name} matching `{}`", one_line(pattern)),
            ArgumentMatcher::Equals { equals } => format!("{name} being `{}`", one_line(equals)),
        })
        .collect();
    match arguments.is_empty() {
        true => format!("every call of {tool}"),
        false => format!("calls of {tool} with {}", arguments.join(" and ")),
    }
}

/// What tells that so many `events` of a channel were missed.
pub(super) fn missed(events: u64) -> String {
    format!("{events} events of this channel missed.")
}

/// What tells how to answer the approval `request` by a command, the beginning of its id naming
/// it - also where it is asked (`Asking`), in the channel talked in.
pub(super) fn how_to_approve(request: &ApprovalRequest) -> String {
    let id = short_id(&request.id.0);
    format!("Type '/approve {id}' or '/deny {id}' to respond.")
}

/// The beginning of `id` the terminal names it by: enough for the commands, which take the
/// beginning of an id (`id_prefix::beginning_with`).
pub(super) fn short_id(id: &Uuid) -> String {
    id.simple().to_string()[..SHORT_ID].to_string()
}

/// The rows that ask the questions of `request`, each with its choices below it.
pub(super) fn question_rows(request: &QuestionRequest) -> Vec<String> {
    let mut rows = Vec::new();
    for (index, question) in request.questions.questions.iter().enumerate() {
        rows.push(question_line(index, question));
        for (number, choice) in question.choices.iter().enumerate() {
            rows.push(format!("   {}) {}", number + 1, choice_words(choice)));
        }
    }
    rows
}

/// What tells how to answer the questions of `request` if they are `put_to_user` - with a
/// warning where they are sensitive -, else that they are put to others.
pub(super) fn how_to_answer(request: &QuestionRequest, put_to_user: bool) -> Vec<String> {
    let id = short_id(&request.id.0);
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
        "They are asked one by one in the channel talked in, else by '/answer {id}'; '/decline \
         {id}' declines them."
    ));
    how
}

/// `text` on one line - its line breaks told as `⏎` -, shortened (`shortened`).
fn one_line(text: &str) -> String {
    shortened(&text.lines().collect::<Vec<_>>().join("⏎"))
}

/// `text` shortened to at most `SHORTENED` characters, the cut marked.
fn shortened(text: &str) -> String {
    match text.char_indices().nth(SHORTENED) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::workspace::testing::make_command_state;
    use caelumora_domain::workspace::CommandState;
    use caelumora_tool_workspace::component::SHELL_PRESENTATION;

    /// A command failed where it could not run or ended by another exit code than 0, a call
    /// where it did not answer.
    #[test]
    fn a_call_failed_by_its_command_or_by_how_it_ended() {
        let call = ToolCall { presentation: SHELL_PRESENTATION, ..make_tool_call("run_command") };
        let failed = |status, content: String| {
            let (_, failed) =
                outcome(&ToolCallResult::of(&call, status, content, Default::default()));
            failed
        };
        let command = |status, exit_code| {
            let state = CommandState { exit_code, ..make_command_state(status) };
            failed(ToolResultStatus::Ok, serde_json::to_string(&state).unwrap())
        };
        assert!(!command(CommandStatus::Completed, Some(0)));
        assert!(command(CommandStatus::Completed, Some(2)));
        assert!(!command(CommandStatus::Running, None));
        assert!(command(CommandStatus::Failed, None));
        assert!(!failed(ToolResultStatus::Ok, "plain".to_string()));
        assert!(failed(ToolResultStatus::Failed, "no shell here".to_string()));
    }
}
