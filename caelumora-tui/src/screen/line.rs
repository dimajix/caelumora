//! The way of drawing `line`: what is printed is written line by line, and what the user types is
//! read by rustyline (issue 569).

use super::{Screen, Typed};
use crate::printed::{ErrorKind, Printed, Said, Saying, Words};
use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::question::QuestionRequest;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use std::io::Error;
use std::thread;
use tokio::sync::mpsc;

/// The way of drawing `line`: each line written once, after those before it.
pub(crate) struct LineScreen {
    /// The lines read by rustyline, in a thread of its own, which begins to read as the first line
    /// is asked for: what is printed before stands before its first prompt.
    input: Option<mpsc::UnboundedReceiver<Result<String, ReadlineError>>>,
}

impl LineScreen {
    /// The way of drawing `line`, yet to read anything.
    pub(crate) fn new() -> Self {
        Self { input: None }
    }
}

impl Screen for LineScreen {
    #[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
    fn print(&mut self, printed: Vec<Printed>) {
        for line in printed.iter().flat_map(lines_of) {
            println!("\r{line}");
        }
    }

    async fn next_input(&mut self) -> Typed {
        let input = self.input.get_or_insert_with(spawn_rustyline_input);
        match input.recv().await {
            Some(Ok(line)) => Typed::Line(line),
            Some(Err(ReadlineError::Interrupted)) => Typed::Interrupted,
            Some(Err(err)) => Typed::Failed(format!("{err:?}")),
            None => Typed::Ended,
        }
    }
}

/// Reads lines with rustyline, in a thread of its own, until reading fails or nobody takes them.
fn spawn_rustyline_input() -> mpsc::UnboundedReceiver<Result<String, ReadlineError>> {
    let (input_tx, input_rx) = mpsc::unbounded_channel::<Result<String, ReadlineError>>();
    thread::spawn(move || {
        let mut rl = match Editor::<(), DefaultHistory>::new() {
            Ok(rl) => rl,
            Err(err) => {
                let _ = input_tx.send(Err(ReadlineError::Io(Error::other(err.to_string()))));
                return;
            }
        };

        loop {
            match rl.readline(">> ") {
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

/// The lines `printed` is written as.
fn lines_of(printed: &Printed) -> Vec<String> {
    match printed {
        Printed::Said { sender, said, saying } => {
            let said = said_line(said);
            let line = match saying {
                Saying::Complete => format!("{sender}> {said}"),
                Saying::Step => format!("[working] {sender}> {said}"),
                Saying::Edited => format!("{sender}> [edited] {said}"),
            };
            vec![line]
        }
        Printed::Approval(request) => approval_lines(request),
        Printed::Questions { request, put_to_user } => questions_lines(request, *put_to_user),
        Printed::Notice(notice) => vec![format!("--- {notice} ---")],
        Printed::Error { kind, message } => vec![format!("[{}] {message}", error_tag(*kind))],
        Printed::Missed { events } => vec![format!("[{events} events missed]")],
        Printed::Plain(line) => vec![line.clone()],
        Printed::Elsewhere { channel, printed } => {
            lines_of(printed).into_iter().map(|line| format!("[{channel}] {line}")).collect()
        }
    }
}

/// What `said` says on a line: its words, the participants they mention highlighted, and what it
/// carries (issue 282).
fn said_line(said: &Said) -> String {
    let words = said.words.as_ref().map(highlighted);
    let attachments = said.attachments.iter().map(|attachment| {
        let name = attachment.name.as_str();
        format!("[{name}, {}, {}]", attachment.media_type, attachment.size_shown())
    });
    let parts: Vec<String> = words.into_iter().chain(attachments).collect();
    parts.join(" ")
}

/// The word `kind` is told by, in brackets before what went wrong.
fn error_tag(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Failed => "error",
        ErrorKind::Rejected => "rejected",
        ErrorKind::Refused => "refused",
    }
}

/// `words` with the participants they mention highlighted.
fn highlighted(words: &Words) -> String {
    let text = &words.text;
    let mut rendered = String::with_capacity(text.len());
    let mut pos = 0;
    for mention in &words.mentions {
        rendered.push_str(&text[pos..mention.start]);
        rendered.push_str("\x1b[1;36m");
        rendered.push_str(&text[mention.clone()]);
        rendered.push_str("\x1b[0m");
        pos = mention.end;
    }
    rendered.push_str(&text[pos..]);
    rendered
}

/// The lines that show `request` for approval, and how to answer it.
fn approval_lines(request: &ApprovalRequest) -> Vec<String> {
    let id = request.id.0;
    let mut lines =
        vec![format!("--- Approval requested (id: {id}) ---"), request.description.clone()];
    if request.explicit {
        lines.push(
            "A rule of this session or channel asks for this call to be decided by a person."
                .to_string(),
        );
    }
    lines.push(format!("Type '/approve {id}' or '/deny {id}' to respond."));
    lines
}

/// The lines that show the questions of `request` (issue 165), and - if they are `put_to_user` -
/// how to answer them.
fn questions_lines(request: &QuestionRequest, put_to_user: bool) -> Vec<String> {
    let id = request.id.0;
    let mut lines =
        vec![format!("--- Questions of {} (id: {id}) ---", request.tool_call.function_name)];
    for (index, question) in request.questions.questions.iter().enumerate() {
        lines.push(format!("{}. [{}] {}", index + 1, question.header, question.question));
        for (number, choice) in question.choices.iter().enumerate() {
            let recommended = if choice.recommended { " (recommended)" } else { "" };
            let description =
                choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
            lines.push(format!(
                "   {}) {}{}{}",
                number + 1,
                choice.label,
                recommended,
                description
            ));
        }
    }
    if put_to_user {
        if request.questions.sensitive {
            lines.push(
                "Sensitive: the answers go to the tool alone - but what you type is shown on this \
                 terminal."
                    .to_string(),
            );
        }
        lines.push(format!(
            "Type '/answer {id}' to answer them one by one, or '/decline {id}'. A message instead \
             declines them."
        ));
    } else {
        lines.push("They are put to others.".to_string());
    }
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
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::content::ContentRef;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Questions;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;

    fn said(words: Option<&str>, attachments: Vec<ContentRef>, saying: Saying) -> Printed {
        let words = words.map(|text| Words { text: text.to_string(), mentions: Vec::new() });
        let said = Said { words, attachments };
        Printed::Said { sender: "helper".to_string(), said, saying }
    }

    /// What was said is written by its sender, a step of a plan and a change marked as such.
    #[test]
    fn what_was_said_is_written_by_its_kind() {
        let said = |words, saying| lines_of(&said(Some(words), Vec::new(), saying));
        assert_eq!(said("hello", Saying::Complete), ["helper> hello"]);
        assert_eq!(said("hm", Saying::Step), ["[working] helper> hm"]);
        assert_eq!(said("now", Saying::Edited), ["helper> [edited] now"]);
    }

    #[test]
    fn a_message_is_written_with_what_it_carries() {
        let chart = make_content_ref("chart.png", b"chart");
        let carrying = |words| lines_of(&said(words, vec![chart.clone()], Saying::Complete));
        assert_eq!(carrying(None), ["helper> [chart.png, image/png, 5 B]"]);
        assert_eq!(carrying(Some("look")), ["helper> look [chart.png, image/png, 5 B]"]);
    }

    #[test]
    fn a_mention_is_highlighted_where_it_stands() {
        let text = "hi @kaya and @lou, not @all".to_string();
        let words = Words { text, mentions: vec![3..8, 13..17] };
        assert_eq!(
            highlighted(&words),
            "hi \x1b[1;36m@kaya\x1b[0m and \x1b[1;36m@lou\x1b[0m, not @all"
        );
    }

    /// What went wrong is written with its kind in brackets, what was missed by its number.
    #[test]
    fn what_went_wrong_is_written_by_its_kind() {
        let error = |kind| Printed::Error { kind, message: "no".to_string() };
        assert_eq!(lines_of(&error(ErrorKind::Failed)), ["[error] no"]);
        assert_eq!(lines_of(&error(ErrorKind::Rejected)), ["[rejected] no"]);
        assert_eq!(lines_of(&error(ErrorKind::Refused)), ["[refused] no"]);
        assert_eq!(lines_of(&Printed::Missed { events: 3 }), ["[3 events missed]"]);
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
        assert_eq!(mine.first(), Some(&format!("--- Questions of ask (id: {id}) ---")));
        assert_eq!(mine.get(1).map(String::as_str), Some("1. [Name] Who?"));
        assert!(mine.last().is_some_and(|line| line.starts_with(&format!("Type '/answer {id}'"))));
        assert!(!mine.join("\n").contains("Sensitive"), "{mine:?}");
        let sensitive = QuestionRequest {
            questions: Questions { sensitive: true, ..request.questions.clone() },
            ..request
        };
        assert!(questions(&sensitive, true).join("\n").contains("Sensitive"));
        assert!(!questions(&sensitive, false).join("\n").contains("Sensitive"), "not the user's");
    }
}
