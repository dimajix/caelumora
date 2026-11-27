//! How the full-screen terminal draws what it prints and what goes on: in lines styled by what
//! they tell (issues 569, 572, 615).

use super::call::{ending_line, ShownCall};
use super::PROMPT;
use crate::activity::{Activity, Place};
use crate::printed::{Printed, Said, Saying, Words};
use crate::screen::words::{
    attachment, error_word, how_to_answer, how_to_approve, missed, question_rows, to_decide,
    EXPLICIT,
};
use crate::under_way::UnderWay;
use caelumora_domain::actor::ActorState;
use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::question::QuestionRequest;
use caelumora_domain::tool::ToolCallResult;
use caelumora_domain::value::name::Name;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use std::time::Instant;

/// The frames the status of an answer under way turns through, one each tenth of a second.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// What the full-screen view keeps of a thing printed: lines, a tool call - an entry of its own -,
/// or how a call ended, which joins its call where that is shown.
pub(super) enum Kept {
    /// Lines; none for what the status line shows instead.
    Lines(Vec<Line<'static>>),
    /// A tool call.
    Call(Box<ShownCall>),
    /// How the call of `result` ended, in the channel `channel_id`: `alone` where its call is not
    /// shown.
    Ending { channel_id: ChannelId, result: Box<ToolCallResult>, alone: Vec<Line<'static>> },
}

/// What the full-screen view keeps of `printed`, told of the channel `elsewhere` where the user
/// talks in another.
pub(super) fn kept(printed: &Printed, elsewhere: Option<&Name>) -> Kept {
    let lines = match printed {
        Printed::Said { sender, said, saying } => said_lines(sender, said, *saying),
        // The status line shows it as long as the answer is under way.
        Printed::Answering { .. } => Vec::new(),
        Printed::ToolCall { channel_id, sender, call } => {
            let channel = elsewhere.cloned();
            let call = ShownCall::new(*channel_id, channel, sender.clone(), (**call).clone());
            return Kept::Call(Box::new(call));
        }
        Printed::ToolResult { channel_id, result } => {
            let alone = told_of(elsewhere, vec![ending_line(result)]);
            let (channel_id, result) = (*channel_id, result.clone());
            return Kept::Ending { channel_id, result, alone };
        }
        Printed::Approval(request) => approval_lines(request),
        Printed::Questions { request, put_to_user } => questions_lines(request, *put_to_user),
        Printed::Notice(notice) => text_lines(notice, Style::new().dim().italic()),
        Printed::Error { kind, message } => {
            text_lines(&format!("{}: {message}", error_word(*kind)), Style::new().red())
        }
        Printed::Missed { events } => vec![Line::styled(missed(*events), Style::new().yellow())],
        Printed::Plain(text) => text_lines(text, Style::new()),
        Printed::Elsewhere { channel, printed } => return kept(printed, Some(channel)),
    };
    Kept::Lines(told_of(elsewhere, lines))
}

/// `lines`, told of the channel `elsewhere` where there is one.
fn told_of(elsewhere: Option<&Name>, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    match elsewhere {
        Some(channel) => of_channel(channel, lines),
        None => lines,
    }
}

/// `lines` told of the channel `channel`, while the user talks in another.
pub(super) fn of_channel(channel: &Name, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let told = |line: Line<'static>| {
        let mut spans = vec![Span::styled(format!("[{channel}] "), Style::new().dim())];
        spans.extend(line.spans);
        Line::from(spans).style(line.style)
    };
    lines.into_iter().map(told).collect()
}

/// The line typed by the user, as it is drawn once sent.
pub(super) fn typed_line(line: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    for (index, row) in line.split('\n').enumerate() {
        let lead = match index {
            0 => PROMPT.to_string(),
            _ => " ".repeat(PROMPT.chars().count()),
        };
        lines.push(Line::from(vec![Span::raw(lead).bold(), Span::raw(row.to_string())]));
    }
    lines
}

/// The lines the answers `under_way` are drawn as at the end of what was printed: what each
/// streamed so far of the message it writes - its reasoning dimmed, then its words.
pub(super) fn live_lines(under_way: &[UnderWay]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for answer in under_way {
        if answer.reasoning.is_empty() && answer.content.is_empty() {
            continue;
        }
        lines.push(Line::default());
        let thinking = Style::new().dim().italic();
        for (index, row) in answer.reasoning.split('\n').enumerate() {
            if answer.reasoning.is_empty() {
                break;
            }
            let lead = if index == 0 { format!("{} thinks: ", answer.name) } else { "  ".into() };
            lines.push(Line::from(vec![
                Span::styled(lead, thinking),
                Span::styled(row.to_string(), thinking),
            ]));
        }
        for (index, row) in answer.content.split('\n').enumerate() {
            if answer.content.is_empty() {
                break;
            }
            let lead = match index {
                0 => Span::styled(format!("{}: ", answer.name), Style::new().bold()),
                _ => Span::raw("  "),
            };
            lines.push(Line::from(vec![lead, Span::raw(row.to_string())]));
        }
    }
    lines
}

/// The status line of the answers under way in the thread followed, `here`, at `now`: of each,
/// who answers, where they stand, and for how long; empty where none is.
pub(super) fn status_line(here: &[UnderWay], now: Instant) -> Line<'static> {
    let mut spans = Vec::new();
    for answer in here {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        let elapsed = now.saturating_duration_since(answer.since);
        let tenths = usize::try_from(elapsed.as_millis() / 100).unwrap_or(0);
        spans.push(Span::styled(
            format!("{} ", SPINNER[tenths % SPINNER.len()]),
            Style::new().cyan(),
        ));
        spans.push(Span::styled(answer.name.clone(), Style::new().bold()));
        let state = state_words(answer.state);
        spans.push(Span::styled(format!(" {state} · {}s", elapsed.as_secs()), Style::new().dim()));
    }
    Line::from(spans)
}

/// The rows of the footer below the line typed, of `activity`, in `width` columns (issue 615):
/// the places the user can be (`places_row`), and the session.
pub(super) fn footer_rows(activity: &Activity, width: u16) -> Vec<Line<'static>> {
    let places = places_row([&activity.channels, &activity.threads], usize::from(width));
    let session = Line::styled(format!(" session {}", activity.session), Style::new().dim());
    vec![places, session]
}

/// The row of the places the user can be, `lists` - the channels followed, then, set apart, the
/// line and the threads of the channel talked in -, in `width` columns: as many as fit, where the
/// user is first, then the places marked, then the rest, each list in its order and telling how
/// many of it were left out.
fn places_row(lists: [&[Place]; 2], width: usize) -> Line<'static> {
    let mut shown = lists.map(|list| vec![false; list.len()]);
    for standing in [Standing::Here, Standing::Marked, Standing::Quiet] {
        for (of, list) in lists.iter().enumerate() {
            for (at, place) in list.iter().enumerate() {
                if Standing::of(place) != standing {
                    continue;
                }
                shown[of][at] = true;
                // Where the user is stands even where it does not fit.
                if standing != Standing::Here && places_line(lists, &shown).width() > width {
                    shown[of][at] = false;
                }
            }
        }
    }
    places_line(lists, &shown)
}

/// How a place stands in the footer, which says how early it is given room and how it is drawn:
/// where the user is, marked, or where nothing goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Here,
    Marked,
    Quiet,
}

impl Standing {
    /// How `place` stands: where the user is - nothing is marked there -, else marked or quiet.
    fn of(place: &Place) -> Self {
        match (place.current, place.is_marked()) {
            (true, _) => Self::Here,
            (false, true) => Self::Marked,
            (false, false) => Self::Quiet,
        }
    }
}

/// The places of `lists` that are `shown`, each list's set apart from the other's, and of each
/// list how many were left out.
fn places_line(lists: [&[Place]; 2], shown: &[Vec<bool>; 2]) -> Line<'static> {
    let dim = Style::new().dim();
    let mut spans = vec![Span::raw(" ")];
    for (list, shown) in lists.iter().zip(shown).filter(|(list, _)| !list.is_empty()) {
        if spans.len() > 1 {
            spans.push(Span::styled("  │  ", dim));
        }
        let places = list.iter().zip(shown).filter(|(_, shown)| **shown);
        for (at, (place, _)) in places.enumerate() {
            if at > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            spans.extend(place_spans(place));
        }
        let left_out = shown.iter().filter(|shown| !**shown).count();
        if left_out > 0 {
            spans.push(Span::styled(format!(" +{left_out}"), dim));
        }
    }
    Line::from(spans)
}

/// `place` as the footer shows it (issue 615, D7): a yellow `!` where something waits for the
/// user, a cyan `●` where an agent is at work, then its name - bold where the user is, dimmed
/// where nothing goes on. The colour is never the only mark.
fn place_spans(place: &Place) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if place.waits {
        spans.push(Span::styled("!", Style::new().yellow()));
    }
    if place.at_work {
        spans.push(Span::styled("●", Style::new().cyan()));
    }
    let style = match Standing::of(place) {
        Standing::Here => Style::new().bold(),
        Standing::Marked => Style::new(),
        Standing::Quiet => Style::new().dim(),
    };
    spans.push(Span::styled(place.name.clone(), style));
    spans
}

/// The words that tell where an agent stands in its answer, `state`.
fn state_words(state: Option<ActorState>) -> &'static str {
    match state {
        None | Some(ActorState::Idle) => "is answering",
        Some(ActorState::Planning) => "is thinking",
        Some(ActorState::ExecutingTools) => "is running tools",
        Some(ActorState::WaitingForUser) => "is waiting for you",
        Some(ActorState::Reflecting) => "is finishing",
        Some(ActorState::Cancelling) => "is cancelling",
        Some(ActorState::CarryingOutCommand) => "is carrying out a command",
        Some(ActorState::Sleeping) => "is asleep",
        Some(ActorState::Busy) => "is busy",
    }
}

/// The lines of what `sender` said in a message of the kind `saying`: a message complete set
/// apart and led by its sender, a step of a plan dimmed, a change marked; what it carries after
/// its words.
fn said_lines(sender: &str, said: &Said, saying: Saying) -> Vec<Line<'static>> {
    let text_style = match saying {
        Saying::Step => Style::new().dim(),
        Saying::Complete | Saying::Edited => Style::new(),
    };
    let mut rows =
        said.words.as_ref().map(|words| word_rows(words, text_style)).unwrap_or_default();
    let carried = said
        .attachments
        .iter()
        .map(|carried| Span::styled(format!(" [{}]", attachment(carried)), Style::new().dim()));
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    if let Some(last) = rows.last_mut() {
        last.extend(carried);
        if saying == Saying::Edited {
            last.push(Span::styled(" (edited)", Style::new().dim()));
        }
    }
    let lead = match saying {
        Saying::Step => Span::styled(format!("{sender} (working): "), Style::new().dim()),
        Saying::Complete | Saying::Edited => {
            Span::styled(format!("{sender}: "), Style::new().bold())
        }
    };
    let mut lines = match saying {
        Saying::Complete => vec![Line::default()],
        Saying::Step | Saying::Edited => Vec::new(),
    };
    for (index, row) in rows.into_iter().enumerate() {
        let first = if index == 0 { lead.clone() } else { Span::raw("  ") };
        lines.push(Line::from([vec![first], row].concat()));
    }
    lines
}

/// The rows of `words`, broken where a line break was said, in `style`, the participants they
/// mention highlighted.
fn word_rows(words: &Words, style: Style) -> Vec<Vec<Span<'static>>> {
    let mention = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut rows = vec![Vec::new()];
    for (part, mentions) in word_parts(words) {
        let style = if mentions { mention } else { style };
        for (index, piece) in part.split('\n').enumerate() {
            if index > 0 {
                rows.push(Vec::new());
            }
            if let (false, Some(row)) = (piece.is_empty(), rows.last_mut()) {
                row.push(Span::styled(piece.to_string(), style));
            }
        }
    }
    rows
}

/// The parts of `words`, in order, each with whether it is the mention of a participant.
fn word_parts(words: &Words) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut pos = 0;
    for mention in &words.mentions {
        parts.push((&words.text[pos..mention.start], false));
        parts.push((&words.text[mention.clone()], true));
        pos = mention.end;
    }
    parts.push((&words.text[pos..], false));
    parts.retain(|(part, _)| !part.is_empty());
    parts
}

/// The lines that show `request` for approval - what is decided whole, a line each of its own -,
/// and how to answer it.
fn approval_lines(request: &ApprovalRequest) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    let decided = to_decide(&request.tool_call);
    for (index, row) in decided.split('\n').enumerate() {
        let lead = match index {
            0 => Span::styled("Approval requested: ", Style::new().yellow().bold()),
            _ => Span::raw("  "),
        };
        lines.push(Line::from(vec![lead, Span::raw(row.to_string())]));
    }
    if request.explicit {
        lines.push(Line::styled(EXPLICIT, Style::new().dim()));
    }
    lines.push(Line::styled(how_to_approve(request), Style::new().dim()));
    lines
}

/// The lines that show the questions of `request`, and how to answer them if they are
/// `put_to_user`.
fn questions_lines(request: &QuestionRequest, put_to_user: bool) -> Vec<Line<'static>> {
    let header = format!("Questions of {}:", request.tool_call.function_name);
    let mut lines = vec![Line::default(), Line::styled(header, Style::new().yellow().bold())];
    lines.extend(question_rows(request).into_iter().map(Line::from));
    let how = how_to_answer(request, put_to_user);
    lines.extend(how.into_iter().map(|how| Line::styled(how, Style::new().dim())));
    lines
}

/// The lines of `text`, broken where it has line breaks, in `style`.
fn text_lines(text: &str, style: Style) -> Vec<Line<'static>> {
    text.split('\n').map(|row| Line::styled(row.to_string(), style)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printed::ErrorKind;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::channel::{ChannelId, ThreadId};
    use caelumora_domain::session::SessionId;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
    use caelumora_domain::work::TurnId;
    use std::time::Duration;

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    /// The lines `printed` is drawn as, whatever the view keeps it as.
    fn styled(printed: &Printed) -> Vec<Line<'static>> {
        match kept(printed, None) {
            Kept::Lines(lines) => lines,
            Kept::Call(call) => call.lines().to_vec(),
            Kept::Ending { alone, .. } => alone,
        }
    }

    /// A message complete is set apart and led by its sender, broken where it has line breaks; a
    /// step of a plan is marked so, a change so.
    #[test]
    fn what_was_said_is_led_by_its_sender() {
        // Both mentions stand on the first line: "hi @kaya @lou".
        let said = |text: &str, saying| Printed::Said {
            sender: "helper".to_string(),
            said: Said {
                words: Some(Words { text: text.to_string(), mentions: vec![3..8, 9..13] }),
                attachments: Vec::new(),
            },
            saying,
        };
        assert_eq!(
            texts(&styled(&said("hi @kaya @lou\nbye", Saying::Complete))),
            ["", "helper: hi @kaya @lou", "  bye"]
        );
        let step = styled(&said("hi @kaya @lou", Saying::Step));
        assert_eq!(texts(&step), ["helper (working): hi @kaya @lou"]);
        assert_eq!(
            texts(&styled(&said("hi @kaya @lou", Saying::Edited))),
            ["helper: hi @kaya @lou (edited)"]
        );
        let mentions = step[0].spans.iter().filter(|span| span.content.starts_with('@'));
        let highlighted = mentions.filter(|span| span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(highlighted.count(), 2, "{step:?}");
    }

    /// Tool calls and how they ended are drawn indented under what was said, an answer begun not
    /// at all, what went wrong with its kind.
    #[test]
    fn tools_errors_and_beginnings_are_drawn_by_what_they_tell() {
        let call = make_tool_call("terminal");
        let channel_id = ChannelId::new();
        let printed = |call: &ToolCall| Printed::ToolCall {
            channel_id,
            sender: "helper".to_string(),
            call: Box::new(call.clone()),
        };
        let called = printed(&call);
        assert_eq!(texts(&styled(&called)), ["  helper → terminal (arguments not shown)"]);
        let described =
            ToolCall { description: Some("Run in the workspace:\nls".to_string()), ..call.clone() };
        assert_eq!(
            texts(&styled(&printed(&described))),
            ["▸ helper → terminal Run in the workspace:⏎ls"],
            "by what its tool says it does, on one line, folded"
        );
        let result =
            ToolCallResult::of(&call, ToolResultStatus::Ok, "done".to_string(), Default::default());
        assert_eq!(
            texts(&styled(&Printed::ToolResult { channel_id, result: Box::new(result) })),
            ["    ⎿ terminal answered: done"]
        );
        assert!(styled(&Printed::Answering { sender: "helper".to_string() }).is_empty());
        let error = Printed::Error { kind: ErrorKind::Refused, message: "no".to_string() };
        assert_eq!(texts(&styled(&error)), ["Refused: no"]);
        assert_eq!(
            texts(&styled(&Printed::Notice("Talking in main now".to_string()))),
            ["Talking in main now"]
        );
    }

    /// An approval is drawn with what is decided whole, a line each of its own, and how to answer
    /// it (issue 586).
    #[test]
    fn an_approval_is_drawn_with_what_is_decided_whole() {
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let tool_call = ToolCall {
            description: Some("Run in the workspace:\nls -la".to_string()),
            ..request.tool_call
        };
        let request = ApprovalRequest { tool_call, ..request };
        let lines = texts(&styled(&Printed::Approval(Box::new(request))));
        assert_eq!(lines[..3], ["", "Approval requested: Run in the workspace:", "  ls -la"]);
    }

    /// What an answer under way streamed is drawn by its agent, reasoning before words; one that
    /// streamed nothing is not drawn.
    #[test]
    fn what_answers_under_way_streamed_is_drawn() {
        let thread = ThreadId::new();
        let began = |name: &str| {
            let agent = ActorId::Agent(AgentId::new());
            UnderWay::began(agent, name.to_string(), thread, TurnId::new(), None)
        };
        let mut helper = began("helper");
        let quiet = began("quiet");
        helper.reasoning = "hm".to_string();
        helper.content = "one\ntwo".to_string();
        assert_eq!(
            texts(&live_lines(&[quiet, helper])),
            ["", "helper thinks: hm", "helper: one", "  two"]
        );
    }

    /// The status line tells who answers, where they stand and for how long - nothing else
    /// (issue 615).
    #[test]
    fn the_status_line_tells_who_answers_where_and_how_long() {
        let now = Instant::now();
        let since = now.checked_sub(Duration::from_secs(12)).unwrap_or(now);
        let agent = ActorId::Agent(AgentId::new());
        let state = Some(ActorState::ExecutingTools);
        let (thread, turn) = (ThreadId::new(), TurnId::new());
        let answer = UnderWay::began(agent, "helper".to_string(), thread, turn, state);
        let told = status_line(&[UnderWay { since, ..answer }], now).to_string();
        assert!(told.ends_with("helper is running tools · 12s"), "{told}");
        assert!(status_line(&[], now).to_string().is_empty());
    }

    /// Issue 615: the footer shows the channels and, set apart, the threads of the channel talked
    /// in - where the user is bold, a place where something waits led by a yellow `!`, one where an
    /// agent is at work by a cyan `●`, one where nothing goes on dimmed -, and the session below;
    /// where they do not all fit, where the user is first, then what is marked, and how many of
    /// each list were left out.
    #[test]
    fn the_footer_shows_where_the_user_is_and_marks_the_other_places() {
        let place = |name: &str, current, at_work, waits| Place {
            name: name.to_string(),
            current,
            at_work,
            waits,
        };
        let activity = Activity {
            session: "here-3f2a91c0".to_string(),
            here: Vec::new(),
            channels: vec![
                place("main", true, false, false),
                place("side", false, true, false),
                place("ops", false, false, true),
                place("dev", false, false, false),
            ],
            threads: vec![
                place("line", true, false, false),
                place("researcher", false, true, false),
            ],
        };
        let rows = footer_rows(&activity, 80);
        assert_eq!(
            texts(&rows),
            [" main · ●side · !ops · dev  │  line · ●researcher", " session here-3f2a91c0"]
        );
        let style_of = |text: &str| -> Style {
            let span = rows[0].spans.iter().find(|span| span.content == text);
            span.map(|span| span.style).unwrap_or_else(|| panic!("{text} in {:?}", rows[0]))
        };
        assert_eq!(style_of("main"), Style::new().bold(), "where the user is");
        assert_eq!(style_of("●"), Style::new().cyan());
        assert_eq!(style_of("!"), Style::new().yellow());
        assert_eq!(style_of("side"), Style::new(), "something goes on there");
        assert_eq!(style_of("dev"), Style::new().dim(), "nothing goes on there");
        assert_eq!(rows[1].style, Style::new().dim(), "the session");

        let narrow = texts(&footer_rows(&activity, 36));
        assert_eq!(narrow[0], " main · ●side · !ops +1  │  line +1", "the marked first");
        let narrower = texts(&footer_rows(&activity, 20));
        assert_eq!(narrower[0], " main +3  │  line +1");
        let narrowest = texts(&footer_rows(&activity, 12));
        assert_eq!(
            narrowest[0], " main +3  │  line +1",
            "where the user is, even where it does not fit"
        );
        let alone = Activity { threads: Vec::new(), ..activity };
        let alone = texts(&footer_rows(&alone, 80));
        assert_eq!(alone[0], " main · ●side · !ops · dev", "no channel talked in, no threads");
    }

    /// The parts of words are told apart by whether they mention a participant, empty ones left
    /// out.
    #[test]
    fn words_are_parted_by_their_mentions() {
        let words = Words { text: "@kaya and @lou".to_string(), mentions: vec![0..5, 10..14] };
        assert_eq!(word_parts(&words), [("@kaya", true), (" and ", false), ("@lou", true)]);
    }
}
