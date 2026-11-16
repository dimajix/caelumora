//! A tool call as the full-screen terminal shows it (issue 586): one entry of what was printed -
//! a head telling who calls what and how it ended, and below it, folded until the user unfolds
//! it, what the call's presentation shows of the call and its result.
//!
//! The terminal knows the presentation `shell`, whose result it reads as the `CommandState` the
//! tool gave its model; every other is shown as `default` - and so is a call of `shell` whose
//! result is no such state, one that failed or was refused.

use super::styled::of_channel;
use crate::screen::words::{arguments, lines_counted, outcome, Called};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolPresentation};
use caelumora_domain::value::name::Name;
use caelumora_domain::workspace::{CommandState, CommandStatus};
use caelumora_tool_workspace::component::SHELL_PRESENTATION;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// How far what a call unfolds to is indented under its head.
const INDENT: &str = "      ";

/// A tool call of a channel, shown with its result once it came: its head and how it ended, and
/// what it unfolds to - folded until the user unfolds it -, the lines it is drawn as kept with it.
#[derive(Debug)]
pub(super) struct ShownCall {
    /// The channel it was called in.
    channel_id: ChannelId,
    /// The channel it is told of, where it was printed while the user talked in another.
    elsewhere: Option<Name>,
    /// Who calls it, by name.
    sender: String,
    call: ToolCall,
    /// How it ended, once it did.
    result: Option<ToolCallResult>,
    /// Whether it shows what it unfolds to.
    unfolded: bool,
    /// The lines it is drawn as, as it stands.
    lines: Vec<Line<'static>>,
}

impl ShownCall {
    /// `call` of the channel `channel_id`, called by `sender` and told of as `elsewhere` where the
    /// user talks in another; folded, without a result yet.
    pub(super) fn new(
        channel_id: ChannelId,
        elsewhere: Option<Name>,
        sender: String,
        call: ToolCall,
    ) -> Self {
        let mut shown = Self {
            channel_id,
            elsewhere,
            sender,
            call,
            result: None,
            unfolded: false,
            lines: Vec::new(),
        };
        shown.lines = shown.drawn();
        shown
    }

    /// Whether `result`, of the channel `channel_id`, is how this call ended: the call waits for
    /// its result and has its id.
    pub(super) fn is_ended_by(&self, channel_id: ChannelId, result: &ToolCallResult) -> bool {
        self.result.is_none()
            && self.channel_id == channel_id
            && self.call.id.is_some()
            && self.call.id == result.id
    }

    /// Takes in how it ended, `result`.
    pub(super) fn end(&mut self, result: ToolCallResult) {
        self.result = Some(result);
        self.lines = self.drawn();
    }

    /// Whether it has anything to unfold.
    pub(super) fn folds(&self) -> bool {
        !self.shown().1.is_empty()
    }

    /// Whether it shows what it unfolds to.
    pub(super) fn is_unfolded(&self) -> bool {
        self.unfolded
    }

    /// Unfolds it, or folds it, as `unfolded` says; answers whether that changed what it shows -
    /// not for one that has nothing to unfold.
    pub(super) fn unfold(&mut self, unfolded: bool) -> bool {
        if self.unfolded == unfolded || !self.folds() {
            return false;
        }
        self.unfolded = unfolded;
        self.lines = self.drawn();
        true
    }

    /// The lines it is drawn as: its head, what it unfolds to where it is unfolded, and how it
    /// ended.
    pub(super) fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    /// The lines it is drawn as now, told of its channel where that is another.
    fn drawn(&self) -> Vec<Line<'static>> {
        let (ending, body) = self.shown();
        let marker = match (body.is_empty(), self.unfolded) {
            (true, _) => "  ",
            (false, false) => "▸ ",
            (false, true) => "▾ ",
        };
        let mut lines = vec![self.head(marker)];
        if self.unfolded {
            lines.extend(body);
        }
        lines.extend(ending);
        match &self.elsewhere {
            Some(channel) => of_channel(channel, lines),
            None => lines,
        }
    }

    /// The line that tells that its sender calls its tool - by what the tool says the call does,
    /// else by what it is called with -, led by `marker`.
    fn head(&self, marker: &str) -> Line<'static> {
        let mut spans = vec![
            Span::styled(format!("{marker}{} → ", self.sender), Style::new().dim()),
            Span::styled(self.call.function_name.clone(), Style::new().bold()),
        ];
        match Called::of(&self.call).on_one_line() {
            Called::Described(description) => spans.push(Span::raw(format!(" {description}"))),
            Called::Bare => {}
            Called::NotShown => {
                spans.push(Span::styled(" (arguments not shown)", Style::new().dim()));
            }
            Called::With(arguments) => {
                spans.push(Span::styled(format!(" {arguments}"), Style::new().dim()));
            }
        }
        Line::from(spans)
    }

    /// What it shows - how it ended, once it did, and what it unfolds to -, as its presentation
    /// shows it where the terminal knows it and it can (`PRESENTATIONS`), else as `default`.
    fn shown(&self) -> Shown {
        let result = self.result.as_ref();
        PRESENTATIONS
            .iter()
            .find(|(presentation, _)| *presentation == self.call.presentation)
            .and_then(|(_, show)| show(&self.call, result))
            .unwrap_or_else(|| default(&self.call, result))
    }
}

/// What a call shows: how it ended, on a line of its own once it did, and what it unfolds to.
type Shown = (Option<Line<'static>>, Vec<Line<'static>>);

/// How a presentation shows a call and its result, where it can.
type Presenter = fn(&ToolCall, Option<&ToolCallResult>) -> Option<Shown>;

/// The presentations the terminal knows beside `default`, by name, as the web UI keeps its own:
/// each shows a call and its result where it can, and leaves it to `default` where not.
const PRESENTATIONS: [(ToolPresentation, Presenter); 1] = [(SHELL_PRESENTATION, shell)];

/// The line that tells how the call of `result` ended - dimmed where it answered, red where not -,
/// shown under its call or alone where its call was not shown.
pub(super) fn ending_line(result: &ToolCallResult) -> Line<'static> {
    let style = match result.status.is_ok() {
        true => Style::new().dim(),
        false => Style::new().red(),
    };
    Line::styled(format!("    ⎿ {} {}", result.tool_name, outcome(result)), style)
}

/// The presentation `default`: how the call ended in words (`outcome`), and unfolded what its tool
/// says it does, its arguments and its result, whole.
fn default(call: &ToolCall, result: Option<&ToolCallResult>) -> Shown {
    let mut body = Vec::new();
    if let Some(description) = &call.description {
        body.extend(rows(description, Style::new()));
    }
    if let Some(arguments) = arguments(call) {
        let written = serde_json::to_string_pretty(arguments).unwrap_or_default();
        body.extend(rows(&written, Style::new().dim()));
    }
    if let Some(result) = result.filter(|result| !result.content.trim().is_empty()) {
        body.extend(rows(&result.content, Style::new()));
    }
    (result.map(ending_line), body)
}

/// The presentation `shell` (issue 251), of a call whose result is the state of its command - none
/// before it came, nor for one that failed or answered something else: where the command stands
/// and how much it printed, and unfolded what its tool says the call does, where that runs over
/// more than a line - which the head shows on one -, and what the command printed, earlier output
/// dropped first said so.
fn shell(call: &ToolCall, result: Option<&ToolCallResult>) -> Option<Shown> {
    let state: CommandState = result
        .filter(|result| result.status.is_ok())
        .and_then(|result| serde_json::from_str(&result.content).ok())?;
    let mut body = Vec::new();
    if let Some(description) = call.description.as_deref().filter(|text| text.contains('\n')) {
        body.extend(rows(description, Style::new()));
    }
    if state.truncated {
        body.push(Line::styled(format!("{INDENT}… earlier output dropped"), Style::new().dim()));
    }
    body.extend(rows(&state.output, Style::new()));
    let stands = match (state.status, state.exit_code) {
        (CommandStatus::Running, _) => format!("still running as {}", state.id),
        (CommandStatus::Completed, Some(code)) => format!("exit code {code}"),
        (CommandStatus::Completed, None) => "completed".to_string(),
        (CommandStatus::Killed, _) => "killed".to_string(),
        (CommandStatus::Failed, _) => "failed".to_string(),
    };
    let failed = state.status == CommandStatus::Failed || state.exit_code.is_some_and(|c| c != 0);
    let style = if failed { Style::new().red() } else { Style::new().dim() };
    let printed = lines_counted(state.output.lines().count());
    let ending = Line::styled(format!("    ⎿ {} {stands}, {printed}", call.function_name), style);
    Some((Some(ending), body))
}

/// The rows of `text`, indented under the head, in `style`.
fn rows(text: &str, style: Style) -> Vec<Line<'static>> {
    text.lines().map(|row| Line::styled(format!("{INDENT}{row}"), style)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::ToolResultStatus;
    use serde_json::json;

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    /// A call of `shell`, `ls -la` run in the workspace.
    fn running_ls() -> ToolCall {
        ToolCall {
            arguments: json!({ "command": "ls -la" }),
            presentation: SHELL_PRESENTATION,
            description: Some("Run in the workspace:\nls -la".to_string()),
            ..make_tool_call("run_command")
        }
    }

    /// The result of `call` that says `content`, ended as `status`.
    fn ended(call: &ToolCall, status: ToolResultStatus, content: &str) -> ToolCallResult {
        ToolCallResult::of(call, status, content.to_string(), Default::default())
    }

    /// A call of `shell` is headed by its command, ends by where its command stands and how much
    /// it printed, and unfolds to what it printed.
    #[test]
    fn a_shell_call_unfolds_to_what_its_command_printed() {
        let call = running_ls();
        let mut shown = ShownCall::new(ChannelId::new(), None, "helper".to_string(), call.clone());
        assert_eq!(
            texts(shown.lines()),
            ["▸ helper → run_command Run in the workspace:⏎ls -la"],
            "its description and arguments to unfold, before it ended, as `default`"
        );
        let state = json!({ "id": "c1", "status": "completed", "exit_code": 0,
            "output": "a\nb\n", "next_offset": 4, "truncated": true });
        shown.end(ended(&call, ToolResultStatus::Ok, &state.to_string()));
        assert_eq!(
            texts(shown.lines()),
            [
                "▸ helper → run_command Run in the workspace:⏎ls -la",
                "    ⎿ run_command exit code 0, 2 lines"
            ],
            "folded"
        );
        assert!(shown.unfold(true));
        assert_eq!(
            texts(shown.lines()),
            [
                "▾ helper → run_command Run in the workspace:⏎ls -la",
                "      Run in the workspace:",
                "      ls -la",
                "      … earlier output dropped",
                "      a",
                "      b",
                "    ⎿ run_command exit code 0, 2 lines"
            ]
        );
        assert!(!shown.unfold(true), "unfolded already");
        assert!(shown.unfold(false) && !shown.is_unfolded());
    }

    /// A call of `shell` that failed, or answered what is no command's state, is shown as
    /// `default`; a call with nothing to unfold has nothing to unfold.
    #[test]
    fn what_is_no_command_s_state_is_shown_as_default() {
        let call = running_ls();
        let mut shown = ShownCall::new(ChannelId::new(), None, "helper".to_string(), call.clone());
        shown.end(ended(&call, ToolResultStatus::Failed, "no shell here"));
        assert!(shown.unfold(true));
        assert_eq!(
            texts(shown.lines()),
            [
                "▾ helper → run_command Run in the workspace:⏎ls -la",
                "      Run in the workspace:",
                "      ls -la",
                "      {",
                "        \"command\": \"ls -la\"",
                "      }",
                "      no shell here",
                "    ⎿ run_command failed: no shell here"
            ]
        );

        let bare = ToolCall { arguments: json!({}), ..make_tool_call("stop") };
        let mut shown = ShownCall::new(ChannelId::new(), None, "helper".to_string(), bare.clone());
        shown.end(ended(&bare, ToolResultStatus::Ok, ""));
        assert!(!shown.unfold(true), "nothing to unfold");
        assert_eq!(texts(shown.lines()), ["  helper → stop", "    ⎿ stop answered"]);
    }

    /// A call is ended by the result of its channel and its id, once.
    #[test]
    fn a_call_is_ended_by_the_result_of_its_channel_and_id() {
        let (channel, other) = (ChannelId::new(), ChannelId::new());
        let call = make_tool_call("look");
        let mut shown = ShownCall::new(channel, None, "helper".to_string(), call.clone());
        let result = ended(&call, ToolResultStatus::Ok, "seen");
        assert!(!shown.is_ended_by(other, &result), "another channel's");
        let another = ToolCallResult { id: Some("call_9".to_string()), ..result.clone() };
        assert!(!shown.is_ended_by(channel, &another), "another call's");
        assert!(shown.is_ended_by(channel, &result));
        shown.end(result.clone());
        assert!(!shown.is_ended_by(channel, &result), "ended once");
    }

    /// A call printed while the user talks in another channel is told of its own.
    #[test]
    fn a_call_elsewhere_is_told_of_its_channel() {
        let side = Name::new("side").unwrap();
        let shown = ShownCall::new(
            ChannelId::new(),
            Some(side),
            "helper".to_string(),
            ToolCall { arguments: json!({}), ..make_tool_call("stop") },
        );
        assert_eq!(texts(shown.lines()), ["[side]   helper → stop"]);
    }
}
