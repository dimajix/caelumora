//! The way of drawing `line`: what is printed is written line by line, and what the user types is
//! read by rustyline (issue 569), which completes it at Tab from what can be typed (`Completing`,
//! issue 579) - listing the candidates at a second Tab, as lines a screen reader reads, and hinting
//! nothing beside the cursor.

use super::words::{
    attachment, error_word, how_to_answer, how_to_approve, missed, offered_words, outcome,
    question_rows, to_decide, what_is_asked, Called, EXPLICIT,
};
use super::{Screen, Typed};
use crate::asking::{Asked, Asking};
use crate::printed::{Printed, Said, Saying};
use crate::typeable::Typeable;
use crate::under_way::UnderWay;
use async_trait::async_trait;
use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::question::QuestionRequest;
use caelumora_domain::tool::{ToolCall, ToolCallResult};
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor, ExternalPrinter, Helper};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use tokio::sync::mpsc;
use tracing::warn;

/// The way of drawing `line`: each line written once, after those before it, above the line being
/// typed.
pub(crate) struct LineScreen {
    /// What writes above the line being typed and keeps it; `None` where the terminal is no TTY,
    /// where lines are written as they come.
    printer: Option<Box<dyn ExternalPrinter + Send>>,
    /// What lets the editor begin to read, until the first line is asked for: what is written
    /// before stands before its first prompt.
    start: Option<std_mpsc::Sender<()>>,
    /// The lines the editor reads, in a thread of its own.
    input: mpsc::UnboundedReceiver<Result<String, ReadlineError>>,
    /// What was shown asked last, written once as it was shown.
    shown: Option<Asking>,
    /// What the editor completes from, as last offered.
    completing: Completing,
}

impl LineScreen {
    /// The way of drawing `line`, yet to read anything.
    ///
    /// # Errors
    /// If the terminal cannot be read from.
    pub(crate) fn new() -> Result<Self, ReadlineError> {
        let config = Config::builder().completion_type(CompletionType::List).build();
        let mut editor = Editor::with_config(config)?;
        let completing = Completing::new();
        editor.set_helper(Some(completing.clone()));
        let printer = editor.create_external_printer().ok();
        let printer = printer.map(|printer| Box::new(printer) as Box<dyn ExternalPrinter + Send>);
        let (start, started) = std_mpsc::channel();
        let input = spawn_rustyline_input(editor, started);
        Ok(Self { printer, start: Some(start), input, shown: None, completing })
    }

    /// Writes `lines` above the line being typed, where there is a printer for it; else, or where
    /// it fails, after what was written.
    #[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
    fn write(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        if let Some(printer) = &mut self.printer {
            let text: String = lines.iter().map(|line| format!("{line}\r\n")).collect();
            match printer.print(text) {
                Ok(()) => return,
                Err(error) => warn!("The terminal could not write above the line typed: {error}"),
            }
        }
        for line in lines {
            println!("\r{line}");
        }
    }
}

#[async_trait]
impl Screen for LineScreen {
    fn print(&mut self, printed: Vec<Printed>) {
        self.write(printed.iter().flat_map(lines_of).collect());
    }

    /// Nothing: an answer is told once as it begins (`Printed::Answering`), and no more until it
    /// says something, so that a screen reader reads what is said, not where an agent stands.
    fn show_under_way(&mut self, _under_way: &[UnderWay]) {}

    /// Writes what is asked once, as it is shown, with its choices numbered and how to answer.
    fn ask(&mut self, asked: Option<&Asked>) {
        let shown = asked.and_then(Asked::shown);
        if self.shown.as_ref() == shown {
            return;
        }
        self.shown = shown.cloned();
        if let Some(asking) = shown {
            self.write(asked_lines(asking));
        }
    }

    /// Hands `typeable` to the editor, which completes from it at the next Tab.
    fn offer(&mut self, typeable: Arc<Typeable>) {
        self.completing.offer(typeable);
    }

    async fn next_input(&mut self) -> Typed {
        if let Some(start) = self.start.take() {
            // Fails only where the thread ended already, which the input then tells.
            let _ = start.send(());
        }
        match self.input.recv().await {
            Some(Ok(line)) => Typed::Line(line),
            Some(Err(ReadlineError::Interrupted)) => Typed::Interrupted,
            Some(Err(err)) => Typed::Failed(format!("{err:?}")),
            None => Typed::Ended,
        }
    }
}

/// Reads lines with `editor`, in a thread of its own, once `started` tells it to begin, until
/// reading fails or nobody takes them.
fn spawn_rustyline_input(
    mut editor: Editor<Completing, DefaultHistory>,
    started: std_mpsc::Receiver<()>,
) -> mpsc::UnboundedReceiver<Result<String, ReadlineError>> {
    let (input_tx, input_rx) = mpsc::unbounded_channel::<Result<String, ReadlineError>>();
    thread::spawn(move || {
        if started.recv().is_err() {
            return;
        }
        loop {
            match editor.readline(">> ") {
                Ok(line) => {
                    if input_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = input_tx.send(Err(err));
                    break;
                }
            }
        }
    });
    input_rx
}

/// What the editor of `line` completes what is typed with (issue 579): what can be typed, as the
/// talk last offered it - shared between the screen, which is offered it, and the editor's thread,
/// which cannot ask the talk.
#[derive(Clone)]
struct Completing {
    typeable: Arc<Mutex<Arc<Typeable>>>,
}

impl Completing {
    /// Completing from nothing, until something is offered.
    fn new() -> Self {
        Self { typeable: Arc::new(Mutex::new(Arc::new(Typeable::default()))) }
    }

    /// Completes from `typeable` from now on.
    fn offer(&self, typeable: Arc<Typeable>) {
        *self.typeable.lock().unwrap_or_else(PoisonError::into_inner) = typeable;
    }
}

impl Completer for Completing {
    type Candidate = String;

    /// What completes `line` up to `pos` (`Typeable::complete`).
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let typeable = Arc::clone(&self.typeable.lock().unwrap_or_else(PoisonError::into_inner));
        let completion = typeable.complete(line, pos);
        Ok((completion.start, completion.candidates))
    }
}

/// No hint beside the cursor: a screen reader would read it as typed.
impl Hinter for Completing {
    type Hint = String;
}

impl Highlighter for Completing {}

impl Validator for Completing {}

impl Helper for Completing {}

/// The lines `printed` is written as: in words, not marked by symbols or colours, which a screen
/// reader reads as they are or not at all.
fn lines_of(printed: &Printed) -> Vec<String> {
    match printed {
        Printed::Said { sender, said, saying } => {
            let said = said_line(said);
            let line = match saying {
                Saying::Complete => format!("{sender}: {said}"),
                Saying::Step => format!("{sender}, working: {said}"),
                Saying::Edited => format!("{sender}, edited: {said}"),
            };
            vec![line]
        }
        Printed::Answering { sender } => vec![format!("{sender} is answering.")],
        Printed::ToolCall { sender, call, .. } => vec![call_line(sender, call)],
        Printed::ToolResult { result, .. } => vec![result_line(result)],
        Printed::Approval(request) => approval_lines(request),
        Printed::Questions { request, put_to_user } => questions_lines(request, *put_to_user),
        Printed::Notice(notice) | Printed::Plain(notice) => vec![notice.clone()],
        Printed::Error { kind, message } => vec![format!("{}: {message}", error_word(*kind))],
        Printed::Missed { events } => vec![missed(*events)],
        Printed::Elsewhere { channel, printed } => {
            lines_of(printed).into_iter().map(|line| format!("In {channel}: {line}")).collect()
        }
    }
}

/// What `said` says on a line: its words as they were said, and what it carries (issue 282).
fn said_line(said: &Said) -> String {
    let words = said.words.as_ref().map(|words| words.text.clone());
    let carried: Vec<String> = said.attachments.iter().map(attachment).collect();
    let carried = (!carried.is_empty()).then(|| format!("attached {}", carried.join("; ")));
    let parts: Vec<String> = words.into_iter().chain(carried).collect();
    parts.join(", ")
}

/// The line that tells that `sender` calls the tool of `call`, and with what.
fn call_line(sender: &str, call: &ToolCall) -> String {
    let tool = &call.function_name;
    match Called::of(call).on_one_line() {
        Called::Described(description) => format!("{sender} calls {tool}: {description}"),
        Called::Bare => format!("{sender} calls {tool}"),
        Called::NotShown => format!("{sender} calls {tool}, its arguments not shown"),
        Called::With(arguments) => format!("{sender} calls {tool} with {arguments}"),
    }
}

/// The line that tells how a call of the tool of `result` ended.
fn result_line(result: &ToolCallResult) -> String {
    format!("{} {}", result.tool_name, outcome(result))
}

/// The lines that show `request` for approval, and how to answer it.
fn approval_lines(request: &ApprovalRequest) -> Vec<String> {
    let id = request.id.0;
    let mut lines = vec![format!("Approval requested, id {id}:"), to_decide(&request.tool_call)];
    if request.explicit {
        lines.push(EXPLICIT.to_string());
    }
    lines.push(how_to_approve(request));
    lines
}

/// The lines that ask `asking`: what is asked, its choices numbered, and how to answer.
fn asked_lines(asking: &Asking) -> Vec<String> {
    let choices = asking.choices();
    let mut lines = vec![what_is_asked(asking)];
    let numbered = choices.iter().enumerate();
    let words = |(index, offered)| format!("   {}) {}", index + 1, offered_words(offered));
    lines.extend(numbered.map(words));
    let how = match (choices.is_empty(), asking.multi_select(), asking.takes_words()) {
        (true, _, _) => "Type your answer, or '-' to decline it.",
        (false, _, false) => "Type the number of your choice.",
        (false, false, true) => {
            "Type the number of your choice, or an answer of your own; '-' declines it."
        }
        (false, true, true) => {
            "Type the numbers of your choices, separated by commas, or an answer of your own; \
             '-' declines it."
        }
    };
    lines.push(how.to_string());
    lines
}

/// The lines that show the questions of `request` (issue 165), and how to answer them if they
/// are `put_to_user`.
fn questions_lines(request: &QuestionRequest, put_to_user: bool) -> Vec<String> {
    let id = request.id.0;
    let mut lines = vec![format!("Questions of {}, id {id}:", request.tool_call.function_name)];
    lines.extend(question_rows(request));
    lines.extend(how_to_answer(request, put_to_user));
    lines
}

#[cfg(test)]
pub(crate) mod testing {
    //! What the tests of the terminal compare what it prints by: the lines `line` writes of it.

    use super::lines_of;
    use crate::printed::Printed;

    /// The lines `printed` is written as, one after the other.
    pub(crate) fn lines(printed: &[Printed]) -> Vec<String> {
        printed.iter().flat_map(lines_of).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printed::{ErrorKind, Words};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::approval::{ArgumentMatcher, ToolMatcher};
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::command::ParameterKind;
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::content::ContentRef;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Questions;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolResultStatus, ToolVisibility};
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use serde_json::json;
    use serde_json::Value;

    /// Issue 579: the editor completes what is typed from what was offered last - the handles of
    /// the participants after an `@` here -, from where the handle begins.
    #[test]
    fn the_editor_completes_from_what_was_offered_last() {
        let completing = Completing::new();
        let history = DefaultHistory::new();
        let completed = |line: &str| completing.complete(line, line.len(), &Context::new(&history));
        assert_eq!(completed("hi @b").unwrap(), (3, Vec::new()), "nobody offered yet");
        let mut offered = Typeable::default();
        offered.add_named(ParameterKind::Participant, ["@bob".to_string(), "@bea".to_string()]);
        completing.offer(Arc::new(offered));
        assert_eq!(completed("hi @b").unwrap(), (3, vec!["@bea".to_string(), "@bob".to_string()]));
    }

    fn said(words: Option<&str>, attachments: Vec<ContentRef>, saying: Saying) -> Printed {
        let words = words.map(|text| Words { text: text.to_string(), mentions: Vec::new() });
        let said = Said { words, attachments };
        Printed::Said { sender: "helper".to_string(), said, saying }
    }

    /// What was said is written after its sender, a step of a plan and a change told as such, in
    /// words.
    #[test]
    fn what_was_said_is_written_by_its_kind() {
        let said = |words, saying| lines_of(&said(Some(words), Vec::new(), saying));
        assert_eq!(said("hello", Saying::Complete), ["helper: hello"]);
        assert_eq!(said("hm", Saying::Step), ["helper, working: hm"]);
        assert_eq!(said("now", Saying::Edited), ["helper, edited: now"]);
    }

    #[test]
    fn a_message_is_written_with_what_it_carries() {
        let chart = make_content_ref("chart.png", b"chart");
        let carrying = |words| lines_of(&said(words, vec![chart.clone()], Saying::Complete));
        assert_eq!(carrying(None), ["helper: attached chart.png, image/png, 5 B"]);
        assert_eq!(carrying(Some("look")), ["helper: look, attached chart.png, image/png, 5 B"]);
        let photo = make_content_ref("photo.png", b"photo");
        let both = lines_of(&said(None, vec![chart.clone(), photo], Saying::Complete));
        assert_eq!(both, ["helper: attached chart.png, image/png, 5 B; photo.png, image/png, 5 B"]);
    }

    /// A mention is written as it was said, without a colour a screen reader cannot tell.
    #[test]
    fn a_mention_is_written_as_said() {
        let words = Words { text: "hi @kaya @lou".to_string(), mentions: vec![3..8, 9..13] };
        let said = Said { words: Some(words), attachments: Vec::new() };
        assert_eq!(said_line(&said), "hi @kaya @lou");
    }

    /// What went wrong is written after its kind, what was missed by its number.
    #[test]
    fn what_went_wrong_is_written_by_its_kind() {
        let error = |kind| Printed::Error { kind, message: "no".to_string() };
        assert_eq!(lines_of(&error(ErrorKind::Failed)), ["Error: no"]);
        assert_eq!(lines_of(&error(ErrorKind::Rejected)), ["Rejected: no"]);
        assert_eq!(lines_of(&error(ErrorKind::Refused)), ["Refused: no"]);
        assert_eq!(lines_of(&Printed::Missed { events: 3 }), ["3 events of this channel missed."]);
    }

    /// What is told of another channel is led by its name, in words.
    #[test]
    fn what_another_channel_tells_is_led_by_its_name() {
        let elsewhere = Printed::Elsewhere {
            channel: Name::new("side").expect("a name"),
            printed: Box::new(Printed::Notice("Talking".to_string())),
        };
        assert_eq!(lines_of(&elsewhere), ["In side: Talking"]);
    }

    /// Questions put to the user are written with how to answer them, and a warning where they
    /// are sensitive; put to others, as such.
    #[test]
    fn questions_are_written_with_how_to_answer_them_if_put_to_the_user() {
        let me = ActorId::User(UserId::new());
        let request = make_question_request(SessionId::new(), ChannelId::new(), AgentId::new(), me);
        let id = request.id.0;
        let questions = |request: &QuestionRequest, put_to_user| {
            lines_of(&Printed::Questions { request: Box::new(request.clone()), put_to_user })
        };
        let theirs = questions(&request, false);
        assert_eq!(theirs.last().map(String::as_str), Some("They are put to others."));
        let mine = questions(&request, true);
        assert_eq!(mine.first(), Some(&format!("Questions of ask, id {id}:")));
        assert_eq!(mine.get(1).map(String::as_str), Some("1. Name: Who?"));
        let short = &id.simple().to_string()[..8];
        assert!(mine.last().is_some_and(|line| line.contains(&format!("'/answer {short}'"))));
        assert!(!mine.join("\n").contains("Sensitive"), "{mine:?}");
        let sensitive = QuestionRequest {
            questions: Questions { sensitive: true, ..request.questions.clone() },
            ..request
        };
        assert!(questions(&sensitive, true).join("\n").contains("Sensitive"));
        assert!(!questions(&sensitive, false).join("\n").contains("Sensitive"), "not the user's");
    }

    /// A tool called is written by what its tool says it does, or else with its arguments in one
    /// line, shortened, or as not shown.
    #[test]
    fn a_tool_called_is_written_by_its_description_or_its_arguments() {
        let call = |arguments| ToolCall { arguments, ..make_tool_call("terminal") };
        let called = |arguments| call_line("helper", &call(arguments));
        assert_eq!(called(Value::Null), "helper calls terminal, its arguments not shown");
        assert_eq!(called(json!({})), "helper calls terminal");
        assert_eq!(
            called(json!({"command": "ls -la"})),
            r#"helper calls terminal with {"command":"ls -la"}"#
        );
        let long = called(json!({"text": "ä".repeat(500)}));
        assert!(long.ends_with('…') && long.chars().count() < 200, "{long}");

        let described = ToolCall {
            description: Some("Run in the workspace:\nls -la".to_string()),
            ..call(json!({"command": "ls -la"}))
        };
        assert_eq!(
            call_line("helper", &described),
            "helper calls terminal: Run in the workspace:⏎ls -la",
            "by what its tool says it does, on one line"
        );
    }

    /// Whoever decides on a call is told what its tool says it does, whole, or else the tool and
    /// its arguments (issue 586).
    #[test]
    fn a_call_to_decide_is_told_by_its_description_or_its_arguments() {
        let call = ToolCall { arguments: json!({"command": "ls"}), ..make_tool_call("terminal") };
        assert_eq!(to_decide(&call), r#"call terminal with {"command":"ls"}"#);
        assert_eq!(to_decide(&ToolCall { arguments: json!({}), ..call.clone() }), "call terminal");
        let described =
            ToolCall { description: Some("Run in the workspace:\nls".to_string()), ..call };
        assert_eq!(to_decide(&described), "Run in the workspace:\nls", "whole");
    }

    /// How a call ended is written by its first line and how many follow, or by why it did not.
    #[test]
    fn how_a_call_ended_is_written_by_its_first_line_or_why_not() {
        let call = make_tool_call("terminal");
        let ended = |status, content: &str| {
            let classification = Default::default();
            result_line(&ToolCallResult::of(&call, status, content.to_string(), classification))
        };
        assert_eq!(ended(ToolResultStatus::Ok, "done"), "terminal answered: done");
        assert_eq!(
            ended(ToolResultStatus::Ok, "\na\n\nb\nc\n"),
            "terminal answered: a (and 2 lines more)"
        );
        assert_eq!(ended(ToolResultStatus::Ok, ""), "terminal answered");
        assert_eq!(
            ended(ToolResultStatus::Failed, "no such file"),
            "terminal failed: no such file"
        );
        assert_eq!(ended(ToolResultStatus::Denied, "denied by a user"), "terminal was denied");
    }

    /// A result not shown in the channel is told as such, not as an empty one.
    #[test]
    fn a_result_not_shown_is_told_so() {
        let call =
            ToolCall { visibility: ToolVisibility::WITHOUT_RESULT, ..make_tool_call("read") };
        let classification = Default::default();
        let hidden = ToolCallResult::of(&call, ToolResultStatus::Ok, String::new(), classification);
        assert_eq!(result_line(&hidden), "read answered, its result not shown");
    }

    /// What is asked is written with its choices numbered and how to answer it.
    #[test]
    fn what_is_asked_is_written_with_its_choices_numbered() {
        let me = ActorId::User(UserId::new());
        let request = make_question_request(SessionId::new(), ChannelId::new(), AgentId::new(), me);
        let asked = asked_lines(&Asking::questions(request));
        assert_eq!(asked, ["1. Name: Who?", "Type your answer, or '-' to decline it."]);
    }

    /// An approval is asked with its choices in words: yes - for the calls alike from now on, as
    /// its tool suggests them, in the channel or the session -, and no.
    #[test]
    fn an_approval_is_asked_with_the_calls_alike_in_words() {
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let shell = ArgumentMatcher::Shell { shell: vec!["ls".to_string()] };
        let arguments = [("command".to_string(), shell)].into_iter().collect();
        let matcher = ToolMatcher { tool: "run_command".to_string(), arguments };
        let request = ApprovalRequest { suggested: Some(matcher), ..request };
        let alike =
            "Yes, and from now on for calls of run_command with command beginning with `ls`";
        assert_eq!(
            asked_lines(&Asking::Approval(request)),
            [
                "Approve? call test_tool".to_string(),
                "   1) Yes".to_string(),
                format!("   2) {alike} in this channel"),
                format!("   3) {alike} in this session"),
                "   4) No".to_string(),
                "Type the number of your choice.".to_string(),
            ]
        );
    }

    /// The calls alike are told on one line, shortened, whatever an argument they compare holds.
    #[test]
    fn the_calls_alike_are_told_on_one_line() {
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let content = ArgumentMatcher::Equals { equals: format!("a\nb{}", "c".repeat(500)) };
        let arguments = [("content".to_string(), content)].into_iter().collect();
        let matcher = ToolMatcher { tool: "write_file".to_string(), arguments };
        let request = ApprovalRequest { suggested: Some(matcher), ..request };
        let asked = asked_lines(&Asking::Approval(request));
        assert!(asked[2].contains("content being `a⏎bccc"), "{asked:?}");
        assert!(asked[2].chars().count() < 260 && asked[2].contains('…'), "{asked:?}");
    }

    #[test]
    fn an_answer_begun_is_told() {
        let answering = Printed::Answering { sender: "helper".to_string() };
        assert_eq!(lines_of(&answering), ["helper is answering."]);
    }
}
