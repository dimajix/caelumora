//! The way of drawing `full`: the terminal's whole window, what was printed scrolled above and the
//! line typed below, as Claude Code draws it (issue 569). Tab completes what is typed from what can
//! be typed, showing what remains above the line (issue 579). A tool call stands with its result,
//! folded: a click on its head unfolds it and folds it again, Ctrl+O every call at once (issue
//! 586). Below the line typed, a footer tells where the user is and what goes on elsewhere (issue
//! 615).

mod call;
mod choosing;
mod clipboard;
mod input;
mod scrollback;
mod selection;
mod styled;

use super::{Screen, Typed};
use crate::activity::Activity;
use crate::asking::Asked;
use crate::printed::Printed;
use crate::typeable::Typeable;
use async_trait::async_trait;
use choosing::{Choosing, Taken};
use clipboard::Clipboard;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use input::InputLine;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use scrollback::Scrollback;
use selection::Selection;
use std::io;
use std::panic;
use std::sync::Arc;
use std::time::{Duration, Instant};
use styled::{footer_rows, kept, live_lines, status_line, typed_line, Kept};
use tokio::time::{self, Interval, MissedTickBehavior};
use tracing::warn;

/// The prompt the line is typed after.
const PROMPT: &str = "> ";

/// How many rows the line typed takes at most; what is typed beyond scrolls within them.
const INPUT_ROWS: u16 = 6;

/// How often the window is drawn at most, and the status line of an answer under way anew.
const FRAME: Duration = Duration::from_millis(50);

/// How many rows the mouse wheel scrolls by.
const WHEEL_ROWS: usize = 3;

/// How many rows the candidates of a completion take at most, above the line typed.
const CANDIDATE_ROWS: u16 = 3;

/// What sets the candidates of a completion apart from each other in a row.
const SPACING: &str = "   ";

/// How many rows the window has at least to show the footer below the line typed (issue 615).
const FOOTER_HEIGHT: u16 = 10;

/// How many rows `line` takes at `width` columns, broken at words as it is drawn - which
/// ratatui's `Paragraph::line_count` counts, under its feature `unstable-rendered-line-info`.
fn wrapped_rows(line: &Line<'static>, width: u16) -> usize {
    Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width.max(1))
}

/// The way of drawing `full`: the terminal taken over until it is dropped, its window drawn as
/// `Window` stands, at most once a frame, and what the user marked in it copied as it was drawn.
pub(crate) struct FullScreen {
    /// The terminal drawn on, in its alternate screen.
    terminal: DefaultTerminal,
    /// What the terminal tells of the keys pressed, the mouse and the window.
    events: EventStream,
    /// What the window shows.
    window: Window,
    /// Where what the user marks is copied to.
    clipboard: Clipboard,
    /// What wakes the window to be drawn anew while answers are under way, and what was drawn too
    /// early to be drawn at once.
    frames: Interval,
    /// Whether something changed since the window was drawn.
    changed: bool,
    /// When the window was drawn last.
    drawn: Instant,
}

/// What the full-screen window shows, and how keys, the mouse and pasted text change it: what was
/// printed and what streams, scrolled by the user alone, its tool calls unfolded and folded by a
/// click on their heads or Ctrl+O; what is asked of the user; the status line of the answers under
/// way; the candidates of a completion; the line typed, which Tab completes; the footer of where
/// the user is; and what the user marks with the mouse, and the choice they click.
#[derive(Debug, Default)]
struct Window {
    /// What was printed, and what streams after it.
    scrollback: Scrollback,
    /// What goes on in the session followed (issues 572, 615).
    activity: Activity,
    /// What is asked of the user, as it is chosen among; told on the rule only, where it is put
    /// aside.
    choosing: Option<Choosing>,
    /// Where what is asked was drawn last, which a click there chooses among.
    asked_area: Rect,
    /// Where what was printed was drawn last, where a click on a tool call's head unfolds it.
    printed_area: Rect,
    /// The line being typed.
    input: InputLine,
    /// What the user marks with the mouse, until a key or the left button is pressed, text pasted,
    /// the wheel turned or the window resized - each of which may move other text under its cells.
    selection: Option<Selection>,
    /// What can be typed, as last offered, which Tab completes the line from.
    typeable: Arc<Typeable>,
    /// The candidates of the last completion where there were several, shown above the line typed
    /// until another key is pressed or text pasted.
    candidates: Vec<String>,
}

impl FullScreen {
    /// Takes the window over: raw input, the alternate screen, the mouse and pasted text - given
    /// back as it is dropped, or as the program panics.
    ///
    /// # Errors
    /// If the terminal cannot be taken over; what was taken over by then is given back.
    pub(crate) fn new() -> io::Result<Self> {
        // Before ratatui's own hook, which gives the terminal back and then calls this one.
        let hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
            hook(info);
        }));
        let terminal = ratatui::try_init()?;
        if let Err(error) = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste) {
            ratatui::restore();
            return Err(error);
        }
        let mut frames = time::interval(FRAME);
        frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
        Ok(Self {
            terminal,
            events: EventStream::new(),
            window: Window::default(),
            clipboard: Clipboard::new(),
            frames,
            changed: true,
            drawn: Instant::now(),
        })
    }

    /// Draws the window, if something changed since it was drawn and it was not drawn too short a
    /// time ago - then the next frame does; and copies what the user marked, as it was drawn, once
    /// they released it.
    fn draw(&mut self) {
        if !self.changed || self.drawn.elapsed() < FRAME {
            return;
        }
        let window = &mut self.window;
        match self.terminal.draw(|frame| window.draw(frame, Instant::now())) {
            Ok(drawn) => {
                if let Some(text) = window.take_selected(drawn.buffer) {
                    self.clipboard.copy(&text);
                }
            }
            Err(error) => warn!("The terminal could not draw its window: {error}"),
        }
        self.changed = false;
        self.drawn = Instant::now();
    }
}

impl Drop for FullScreen {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
    }
}

#[async_trait]
impl Screen for FullScreen {
    fn print(&mut self, printed: Vec<Printed>) {
        for printed in &printed {
            self.window.print(printed);
        }
        self.changed = true;
    }

    fn show_activity(&mut self, activity: &Activity) {
        self.window.show_activity(activity);
        self.changed = true;
    }

    /// Completes from `typeable` at the next Tab.
    fn offer(&mut self, typeable: Arc<Typeable>) {
        self.window.offer(typeable);
    }

    fn ask(&mut self, asked: Option<&Asked>) {
        self.changed |= self.window.ask(asked);
    }

    async fn next_input(&mut self) -> Typed {
        loop {
            self.draw();
            tokio::select! {
                event = self.events.next() => match event {
                    Some(Ok(event)) => {
                        self.changed = true;
                        let height = self.terminal.size().map_or(20, |size| size.height);
                        if let Some(typed) =
                            self.window.take_event(event, usize::from(height / 2))
                        {
                            self.draw();
                            return typed;
                        }
                    }
                    Some(Err(error)) => return Typed::Failed(error.to_string()),
                    None => return Typed::Ended,
                },
                _ = self.frames.tick() => {
                    // The status line tells for how long an answer is under way.
                    self.changed |= !self.window.activity.here.is_empty();
                }
            }
        }
    }
}

impl Window {
    /// Shows `printed` as the view keeps it (`kept`): lines, a tool call as an entry of its own,
    /// or how one ended - joining its call where that is shown.
    fn print(&mut self, printed: &Printed) {
        match kept(printed, None) {
            Kept::Lines(lines) => self.scrollback.push(lines),
            Kept::Call(call) => self.scrollback.push_call(*call),
            Kept::Ending { channel_id, result, alone } => {
                if !self.scrollback.end_call(channel_id, &result) {
                    self.scrollback.push(alone);
                }
            }
        }
    }

    /// Completes the line typed from `typeable` at the next Tab.
    fn offer(&mut self, typeable: Arc<Typeable>) {
        self.typeable = typeable;
    }

    /// Shows what goes on, `activity`: what the answers of the thread followed streamed after what
    /// was printed, in the status line where they stand, and in the footer where the user is and
    /// where else agents are at work or something waits for them (issues 572, 615).
    fn show_activity(&mut self, activity: &Activity) {
        self.scrollback.set_live(live_lines(&activity.here));
        self.activity = activity.clone();
    }

    /// Shows what is asked, `asked` - waiting only where it is put aside -, chosen among anew where
    /// something else is asked; answers whether that changed what is shown.
    fn ask(&mut self, asked: Option<&Asked>) -> bool {
        let showing = self.choosing.as_ref().map(Choosing::asked);
        if showing == asked {
            return false;
        }
        let same = showing.map(Asked::asking) == asked.map(Asked::asking);
        match (self.choosing.as_mut(), asked) {
            (Some(choosing), Some(asked)) if same => choosing.show_as(asked.clone()),
            _ => self.choosing = asked.cloned().map(Choosing::new),
        }
        true
    }

    /// Takes in `event`, a page being `page` rows, and answers what the user typed if it ends a
    /// line or the input.
    fn take_event(&mut self, event: Event, page: usize) -> Option<Typed> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.selection = None;
                self.take_key(&key, page)
            }
            Event::Mouse(mouse) => self.take_mouse(&mouse),
            Event::Paste(text) => {
                self.selection = None;
                self.candidates.clear();
                self.input.paste(&text);
                None
            }
            Event::Resize(..) => {
                self.selection = None;
                None
            }
            _ => None,
        }
    }

    /// Takes in `mouse`: the wheel scrolls, and drops what is marked; the left button marks what it
    /// is dragged over, and a click with it unfolds or chooses what it is on
    /// (`Window::take_click`) - answered where that ends what is asked.
    fn take_mouse(&mut self, mouse: &MouseEvent) -> Option<Typed> {
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.selection = None;
                self.scrollback.scroll_up(WHEEL_ROWS);
            }
            MouseEventKind::ScrollDown => {
                self.selection = None;
                self.scrollback.scroll_down(WHEEL_ROWS);
            }
            MouseEventKind::Down(MouseButton::Left) => self.selection = Some(Selection::new(at)),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = &mut self.selection {
                    selection.drag_to(at);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let selection = self.selection.as_mut()?;
                if !selection.release(at) {
                    self.selection = None;
                    return self.take_click(at);
                }
            }
            _ => {}
        }
        None
    }

    /// Takes in a click at `at`: one on the head of a tool call unfolds or folds it
    /// (`Scrollback::unfold_at`), one on a choice of what is asked chooses it
    /// (`Choosing::take_click`) - answered where that ends what is asked. Unlike a digit, which is
    /// typed where something is, a click chooses also then: it is no text.
    fn take_click(&mut self, at: Position) -> Option<Typed> {
        let printed = self.printed_area;
        if printed.contains(at) {
            self.scrollback.unfold_at(at.y - printed.y);
            return None;
        }
        let area = self.asked_area;
        let choosing = self.choosing.as_mut().filter(|_| area.contains(at))?;
        match choosing.take_click(at.y - area.y, area.width) {
            Taken::Typed(typed) => Some(typed),
            Taken::Passed | Taken::Kept => None,
        }
    }

    /// Once the user released what they marked, its text as `buffer`, the window drawn, shows it;
    /// answered once.
    fn take_selected(&mut self, buffer: &Buffer) -> Option<String> {
        self.selection.as_mut()?.take_released(buffer)
    }

    /// Takes in `key`, a page being `page` rows: what ends the program or the input, scrolls,
    /// chooses among what is asked while nothing is typed (`Choosing::take_key`), completes the
    /// line typed (Tab, `InputLine::complete`) - showing the candidates where there are several,
    /// until another key is pressed -, or edits it - and answers the line where it ends it, shown
    /// as sent and the end of what was printed followed.
    fn take_key(&mut self, key: &KeyEvent, page: usize) -> Option<Typed> {
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            self.candidates = self.input.complete(&self.typeable);
            return None;
        }
        self.candidates.clear();
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Shift and the arrows scroll, also while something is asked.
        let plain = key.modifiers.is_empty();
        if let Some(choosing) = self.choosing.as_mut().filter(|_| plain && self.input.is_empty()) {
            match choosing.take_key(key) {
                Taken::Passed => {}
                Taken::Kept => return None,
                Taken::Typed(typed) => return Some(typed),
            }
        }
        match key.code {
            KeyCode::Char('c') if control => return Some(Typed::Interrupted),
            KeyCode::Char('d') if control && self.input.is_empty() => return Some(Typed::Ended),
            KeyCode::PageUp => self.scrollback.scroll_up(page),
            KeyCode::PageDown => self.scrollback.scroll_down(page),
            KeyCode::Up if shift => self.scrollback.scroll_up(1),
            KeyCode::Down if shift => self.scrollback.scroll_down(1),
            KeyCode::End if control => self.scrollback.follow_end(),
            KeyCode::Char('o') if control => self.scrollback.unfold_all(),
            _ => {
                let line = self.input.take_key(key)?;
                self.scrollback.push(typed_line(&line));
                self.scrollback.follow_end();
                return Some(Typed::Line(line));
            }
        }
        None
    }

    /// Draws the window on `frame` at `now`: what was printed and what streams; what is asked; the
    /// status line where answers are under way; the candidates of a completion, in the room left
    /// (`candidate_rows`); a rule, which tells where the user scrolled up; the line typed, with
    /// the cursor in it; below a rule of its own the footer, in a window of `FOOTER_HEIGHT` rows
    /// at least, where everything else still fits; and over all, what the user marks.
    fn draw(&mut self, frame: &mut Frame, now: Instant) {
        let area = frame.area();
        let (rows, cursor) = self.input.rows(PROMPT, area.width);
        let input_rows = u16::try_from(rows.len()).unwrap_or(u16::MAX).min(INPUT_ROWS);
        let status_rows = u16::from(!self.activity.here.is_empty());
        let asked_rows = self.choosing.as_ref().map_or(0, |choosing| choosing.height(area.width));
        let asked_rows = u16::try_from(asked_rows).unwrap_or(u16::MAX).min(area.height / 2);
        // The line typed, its rule, what is asked, the status line and a row of what was printed.
        let taken = input_rows + 1 + asked_rows + status_rows + 1;
        // The footer gives way where it would push any of them out of the window.
        let footer = footer_rows(&self.activity, area.width);
        let footer_taken = u16::try_from(footer.len() + 1).unwrap_or(u16::MAX);
        let fits = area.height >= FOOTER_HEIGHT && taken + footer_taken <= area.height;
        let footer = if fits { footer } else { Vec::new() };
        let footer_rows = u16::try_from(footer.len()).unwrap_or(u16::MAX);
        let footer_rule = u16::from(!footer.is_empty());
        // What is left: the candidates never push the line typed out of the window.
        let room = area.height.saturating_sub(taken + footer_rule + footer_rows);
        let candidates =
            candidate_rows(&self.candidates, area.width, usize::from(CANDIDATE_ROWS.min(room)));
        let candidate_rows = u16::try_from(candidates.len()).unwrap_or(u16::MAX);
        let [printed, asking, status, offered, rule, typed, below, footed] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(asked_rows),
            Constraint::Length(status_rows),
            Constraint::Length(candidate_rows),
            Constraint::Length(1),
            Constraint::Length(input_rows),
            Constraint::Length(footer_rule),
            Constraint::Length(footer_rows),
        ])
        .areas(area);
        self.asked_area = asking;
        self.printed_area = printed;

        let (seen, above) = self.scrollback.visible(printed.width, printed.height);
        let seen = Paragraph::new(seen).wrap(Wrap { trim: false }).scroll((above, 0));
        frame.render_widget(seen, printed);
        if let Some(choosing) = &self.choosing {
            frame.render_widget(Paragraph::new(choosing.rows()).wrap(Wrap { trim: false }), asking);
        }
        frame.render_widget(status_line(&self.activity.here, now), status);
        frame.render_widget(Paragraph::new(candidates), offered);
        let waits = self.choosing.as_ref().and_then(Choosing::waiting);
        frame.render_widget(rule_line(self.scrollback.below(), waits, rule), rule);
        frame.render_widget(rule_line(0, None, below), below);
        frame.render_widget(Paragraph::new(footer), footed);

        // The rows about the cursor, where the line takes more than are shown.
        let first = cursor.y.saturating_sub(input_rows.saturating_sub(1));
        let shown: Vec<Line> = rows.into_iter().skip(usize::from(first)).map(Line::from).collect();
        frame.render_widget(Paragraph::new(shown), typed);
        frame.set_cursor_position(Position::new(typed.x + cursor.x, typed.y + cursor.y - first));
        if let Some(selection) = &self.selection {
            selection.highlight(frame.buffer_mut());
        }
    }
}

/// The rows `candidates` of a completion are shown in above the line typed, dim: as many whole
/// ones a row as fit in `width` columns, set apart from each other, in at most `most` rows - the
/// last telling how many more there are, where they do not all fit.
fn candidate_rows(candidates: &[String], width: u16, most: usize) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let taken =
        |row: &[String]| -> usize { row.iter().map(|c| c.chars().count() + SPACING.len()).sum() };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for candidate in candidates {
        let fits = rows.last().is_some_and(|row| taken(row) + candidate.chars().count() <= width);
        match rows.last_mut() {
            Some(row) if fits => row.push(candidate.clone()),
            _ => rows.push(vec![candidate.clone()]),
        }
    }
    if rows.len() > most {
        rows.truncate(most);
        let before: usize = rows.iter().rev().skip(1).map(Vec::len).sum();
        if let Some(row) = rows.last_mut() {
            // The last row shortened until it holds how many more there are.
            let more = |row: &[String]| format!("+{} more", candidates.len() - before - row.len());
            while !row.is_empty() && taken(row) + more(row).chars().count() > width {
                row.pop();
            }
            let told = more(row);
            row.push(told);
        }
    }
    let dim = Style::new().dim();
    rows.into_iter().map(|row| Line::styled(row.join(SPACING), dim)).collect()
}

/// The rule between what was printed and the line typed, across `area`: how many rows lie
/// `below` where the user scrolled up, and how to follow the end again; and what `waits`, put
/// aside.
fn rule_line(below: usize, waits: Option<String>, area: Rect) -> Line<'static> {
    let scrolled = (below > 0).then(|| format!("↓ {below} rows below · Ctrl+End follows the end"));
    let told: Vec<String> = scrolled.into_iter().chain(waits).collect();
    let told = match told.is_empty() {
        true => String::new(),
        false => format!(" {} ", told.join(" · ")),
    };
    let width = usize::from(area.width);
    let lead = 2.min(width);
    let rest = width.saturating_sub(lead + told.chars().count());
    let rule = format!("{}{told}{}", "─".repeat(lead), "─".repeat(rest));
    Line::styled(rule, Style::new().dim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Place;
    use crate::asking::Asking;
    use crate::under_way::UnderWay;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::approval::ApprovalRequest;
    use caelumora_domain::channel::{ChannelId, ThreadId};
    use caelumora_domain::command::ParameterKind;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::{Choice, Question, QuestionRequest, Questions};
    use caelumora_domain::session::SessionId;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_domain::work::TurnId;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;
    use serde_json::json;

    /// `window` drawn on a terminal of `width` and `height`: its rows, and where the cursor
    /// stands.
    fn drawn(window: &mut Window, width: u16, height: u16) -> (Vec<String>, Position) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal.draw(|frame| window.draw(frame, Instant::now())).expect("drawn");
        let buffer = terminal.backend().buffer();
        let rows = (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .map(|row| row.trim_end().to_string())
            .collect();
        (rows, terminal.get_cursor_position().expect("a cursor"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(window: &mut Window, text: &str) -> Option<Typed> {
        for c in text.chars() {
            window.take_key(&key(KeyCode::Char(c)), 5);
        }
        window.take_key(&key(KeyCode::Enter), 5)
    }

    /// Issue 579: Tab completes the line typed from what was offered; where several candidates
    /// remain, they are shown above the line, until another key is pressed or text pasted.
    #[test]
    fn tab_completes_the_line_and_shows_the_candidates_left_until_another_key() {
        let mut window = Window::default();
        let mut typeable = Typeable::default();
        typeable.add_named(ParameterKind::Participant, ["@alice", "@bob"].map(String::from));
        window.offer(Arc::new(typeable));
        for c in "hi @".chars() {
            window.take_key(&key(KeyCode::Char(c)), 5);
        }
        window.take_key(&key(KeyCode::Tab), 5);
        let (rows, _) = drawn(&mut window, 40, 5);
        assert_eq!(rows[2], "@alice   @all   @bob   @channel", "{rows:?}");
        assert_eq!(rows[3], "─".repeat(40));
        assert_eq!(rows[4], "> hi @");
        window.take_key(&key(KeyCode::Char('b')), 5);
        let (rows, _) = drawn(&mut window, 40, 5);
        assert_eq!(rows[2], "", "another key ends them: {rows:?}");
        window.take_key(&key(KeyCode::Tab), 5);
        let (rows, cursor) = drawn(&mut window, 40, 5);
        assert_eq!(rows[3], "─".repeat(40), "none shown for one: {rows:?}");
        assert_eq!(rows[4], "> hi @bob");
        assert_eq!(cursor, Position::new(9, 4));
        window.take_key(&key(KeyCode::Char(' ')), 5);
        window.take_key(&key(KeyCode::Char('@')), 5);
        window.take_key(&key(KeyCode::Tab), 5);
        window.take_event(Event::Paste("x".to_string()), 5);
        assert!(window.candidates.is_empty(), "text pasted ends them");
    }

    /// Issue 579: the candidates are shown whole, as many a row as fit, in at most so many rows as
    /// there is room for, the last telling how many more there are.
    #[test]
    fn the_candidates_are_shown_whole_and_how_many_more_there_are() {
        let shown = |candidates: &[&str], width, most| -> Vec<String> {
            let candidates: Vec<String> = candidates.iter().map(|c| c.to_string()).collect();
            let rows = candidate_rows(&candidates, width, most);
            rows.iter().map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
        };
        assert_eq!(shown(&["session new", "session list"], 20, 3), ["session new", "session list"]);
        assert_eq!(shown(&["ab", "cd", "ef"], 20, 3), ["ab   cd   ef"]);
        assert_eq!(shown(&["abcd", "efgh", "ijkl", "mnop"], 11, 2), ["abcd   efgh", "ijkl   mnop"]);
        assert_eq!(
            shown(&["abcd", "efgh", "ijkl", "mnop", "qrst"], 11, 2),
            ["abcd   efgh", "+3 more"]
        );
        assert_eq!(shown(&["ab", "cd", "ef", "gh"], 12, 1), ["ab   +3 more"]);
        assert!(shown(&["ab", "cd"], 20, 0).is_empty(), "no room, none shown");
        let mut window = Window::default();
        let mut typeable = Typeable::default();
        typeable.add_named(ParameterKind::Participant, ["@alice", "@bob"].map(String::from));
        window.offer(Arc::new(typeable));
        window.take_key(&key(KeyCode::Char('@')), 5);
        window.take_key(&key(KeyCode::Tab), 5);
        let (rows, cursor) = drawn(&mut window, 8, 4);
        assert_eq!(rows[3], "> @", "the line typed keeps its row: {rows:?}");
        assert_eq!(cursor, Position::new(3, 3));
    }

    /// The window shows what was printed above, the status line of an answer under way, the rule,
    /// and the line typed with the cursor after it.
    #[test]
    fn the_window_shows_what_was_printed_the_status_and_the_line_typed() {
        let mut window = Window::default();
        window.print(&Printed::Notice("Talking in main now".to_string()));
        let agent = ActorId::Agent(AgentId::new());
        let thread = ThreadId::line(ChannelId::new());
        let answer = UnderWay::began(agent, "helper".to_string(), thread, TurnId::new(), None);
        window.show_activity(&Activity { here: vec![answer], ..Activity::default() });
        for c in "hi".chars() {
            window.take_key(&key(KeyCode::Char(c)), 5);
        }
        let (rows, cursor) = drawn(&mut window, 30, 6);
        assert_eq!(rows[0], "Talking in main now");
        assert!(rows[3].ends_with("helper is answering · 0s"), "{rows:?}");
        assert_eq!(rows[4], "─".repeat(30));
        assert_eq!(rows[5], "> hi");
        assert_eq!(cursor, Position::new(4, 5));
    }

    /// Issue 615: below the line typed, set apart by a rule, the footer tells where the user is,
    /// what goes on elsewhere and the session; nothing under way in the thread followed, there is
    /// no status line. A window too low for it shows none.
    #[test]
    fn the_window_shows_the_footer_below_the_line_typed() {
        let mut window = Window::default();
        window.print(&Printed::Notice("Talking in main now".to_string()));
        let place = |name: &str, current, at_work| Place {
            name: name.to_string(),
            current,
            at_work,
            waits: false,
        };
        window.show_activity(&Activity {
            session: "here".to_string(),
            here: Vec::new(),
            channels: vec![place("main", true, false), place("side", false, true)],
            threads: vec![place("line", true, false)],
        });
        let (rows, cursor) = drawn(&mut window, 30, FOOTER_HEIGHT);
        assert_eq!(rows[0], "Talking in main now");
        assert_eq!(rows[5], "─".repeat(30), "the rule above the line typed: {rows:?}");
        assert_eq!(rows[6], ">", "the line typed, empty");
        assert_eq!(rows[7], "─".repeat(30), "the rule below it");
        assert_eq!(rows[8], " main · ●side  │  line");
        assert_eq!(rows[9], " session here");
        assert_eq!(cursor, Position::new(2, 6));

        let (rows, cursor) = drawn(&mut window, 30, FOOTER_HEIGHT - 1);
        assert_eq!(rows[usize::from(FOOTER_HEIGHT) - 2], ">", "the line typed last: {rows:?}");
        assert_eq!(cursor, Position::new(2, FOOTER_HEIGHT - 2));

        // What is asked takes the room: the footer gives way, the line typed keeps its row.
        window.ask(Some(&Asked::Shown(several(&["a", "b", "c", "d"]))));
        let (rows, cursor) = drawn(&mut window, 60, FOOTER_HEIGHT);
        let last = usize::from(FOOTER_HEIGHT) - 1;
        assert_eq!(rows[last], ">", "the line typed last: {rows:?}");
        assert!(!rows.iter().any(|row| row.contains("session here")), "{rows:?}");
        assert_eq!(cursor, Position::new(2, FOOTER_HEIGHT - 1));
    }

    /// A line typed is answered as it ends, and shown as sent; scrolled up, the rule tells how
    /// far, and sending a line follows the end again.
    #[test]
    fn a_line_sent_is_shown_and_follows_the_end() {
        let mut window = Window::default();
        let printed = (0..10).map(|n| Printed::Plain(format!("line {n}")));
        printed.for_each(|printed| window.print(&printed));
        window.take_key(&key(KeyCode::PageUp), 5);
        let (rows, _) = drawn(&mut window, 40, 6);
        assert_eq!(rows[3], "line 4", "five rows up from line 9: {rows:?}");
        assert!(rows[4].contains("5 rows below"), "{rows:?}");

        assert_eq!(typed(&mut window, "hello"), Some(Typed::Line("hello".to_string())));
        let (rows, _) = drawn(&mut window, 40, 6);
        assert_eq!(rows[3], "> hello", "shown as sent, the end followed: {rows:?}");
        assert_eq!(rows[4], "─".repeat(40));
    }

    /// An approval waiting for the user.
    fn approval() -> Asking {
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let tool_call = ToolCall { description: Some("Run ls".to_string()), ..request.tool_call };
        Asking::Approval(ApprovalRequest { tool_call, ..request })
    }

    /// What is asked is shown above the status line, its choices below it, the cursor on the
    /// first; the arrows move it, Enter or a digit chooses, while nothing is typed - else the line
    /// typed takes the keys.
    #[test]
    fn what_is_asked_is_chosen_among_by_keys() {
        let mut window = Window::default();
        let asking = approval();
        assert!(window.ask(Some(&Asked::Shown(asking.clone()))));
        assert!(!window.ask(Some(&Asked::Shown(asking.clone()))), "nothing changed");
        let (rows, _) = drawn(&mut window, 60, 8);
        assert_eq!(&rows[2..6], ["Approve? Run ls", "❯ 1. Yes", "  2. No", rows[5].as_str()]);
        assert!(rows[5].starts_with("↑↓ choose · Enter answers · Esc puts it aside"), "{rows:?}");

        assert_eq!(window.take_key(&key(KeyCode::Down), 5), None);
        assert_eq!(window.take_key(&key(KeyCode::Enter), 5), Some(Typed::Chosen(vec![1])));
        assert_eq!(window.take_key(&key(KeyCode::Char('1')), 5), Some(Typed::Chosen(vec![0])));
        window.take_key(&key(KeyCode::Char('x')), 5);
        assert_eq!(typed(&mut window, "1"), Some(Typed::Line("x1".to_string())), "typed on");
    }

    /// Esc puts what is asked aside: the rule tells that it waits, and Esc takes it up again.
    #[test]
    fn what_is_put_aside_waits_on_the_rule() {
        let mut window = Window::default();
        let asking = approval();
        window.ask(Some(&Asked::Shown(asking.clone())));
        assert_eq!(window.take_key(&key(KeyCode::Esc), 5), Some(Typed::PutAside));
        window.ask(Some(&Asked::Aside(asking.clone())));
        let (rows, _) = drawn(&mut window, 60, 4);
        assert!(rows[2].contains("an approval waits · Esc shows it"), "{rows:?}");
        assert_eq!(window.take_key(&key(KeyCode::Char('1')), 5), None, "typed, not chosen");
        window.take_key(&key(KeyCode::Backspace), 5);
        assert_eq!(window.take_key(&key(KeyCode::Esc), 5), Some(Typed::TakenUp));
    }

    /// A question of which several of the choices `labels` may be taken, the choice `b`
    /// recommended.
    fn several(labels: &[&str]) -> Asking {
        let me = ActorId::User(UserId::new());
        let request = make_question_request(SessionId::new(), ChannelId::new(), AgentId::new(), me);
        let choice = |label: &&str| Choice {
            label: label.to_string(),
            description: None,
            recommended: *label == "b",
        };
        let question = Question {
            header: "Pick".to_string(),
            question: "Which?".to_string(),
            choices: labels.iter().map(choice).collect(),
            multi_select: true,
        };
        let questions = Questions { questions: vec![question], ..request.questions.clone() };
        Asking::questions(QuestionRequest { questions, ..request })
    }

    /// Where several choices may be taken, digits and Space check them, and Enter takes those
    /// checked.
    #[test]
    fn several_choices_are_checked_and_taken_together() {
        let mut window = Window::default();
        window.ask(Some(&Asked::Shown(several(&["a", "b", "c"]))));
        let (rows, _) = drawn(&mut window, 60, 10);
        assert!(rows.contains(&"❯ [ ] 2. b (recommended)".to_string()), "{rows:?}");
        window.take_key(&key(KeyCode::Char('3')), 5);
        window.take_key(&key(KeyCode::Up), 5);
        window.take_key(&key(KeyCode::Up), 5);
        window.take_key(&key(KeyCode::Char(' ')), 5);
        let (rows, _) = drawn(&mut window, 60, 10);
        assert!(rows.contains(&"❯ [x] 1. a".to_string()), "{rows:?}");
        assert_eq!(window.take_key(&key(KeyCode::Enter), 5), Some(Typed::Chosen(vec![0, 2])));
    }

    /// Ctrl+C interrupts - a command under way, else the program -, Ctrl+D ends the input where
    /// nothing is typed.
    #[test]
    fn control_keys_end_the_program_or_the_input() {
        let mut window = Window::default();
        let control = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(window.take_key(&control('c'), 5), Some(Typed::Interrupted));
        window.take_key(&key(KeyCode::Char('x')), 5);
        assert_eq!(window.take_key(&control('d'), 5), None, "something typed");
        window.take_key(&key(KeyCode::Backspace), 5);
        assert_eq!(window.take_key(&control('d'), 5), Some(Typed::Ended));
    }

    /// `window` drawn on `terminal` as `FullScreen` draws it: whether a cell shows a marking, and
    /// what is copied of it.
    fn drawn_marking(
        terminal: &mut Terminal<TestBackend>,
        window: &mut Window,
    ) -> (bool, Option<String>) {
        let drawn = terminal.draw(|frame| window.draw(frame, Instant::now())).expect("drawn");
        let shown =
            drawn.buffer.content.iter().any(|cell| cell.modifier.contains(Modifier::REVERSED));
        (shown, window.take_selected(drawn.buffer))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    /// Dragging over what was printed marks it, shown reversed, and once released its text is
    /// answered as drawn, once; a key drops the marking, as the wheel does, and a click marks
    /// nothing.
    #[test]
    fn what_is_dragged_over_is_marked_and_answered_as_drawn() {
        let mut window = Window::default();
        let printed = ["first line", "second line"].map(|text| Printed::Plain(text.to_string()));
        printed.iter().for_each(|printed| window.print(printed));
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).expect("a test terminal");
        let left = MouseButton::Left;
        window.take_event(mouse(MouseEventKind::Down(left), 6, 0), 5);
        window.take_event(mouse(MouseEventKind::Drag(left), 2, 1), 5);
        window.take_event(mouse(MouseEventKind::Up(left), 5, 1), 5);
        let drawn = terminal.draw(|frame| window.draw(frame, Instant::now())).expect("drawn");
        assert!(drawn.buffer[(6, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!drawn.buffer[(5, 0)].modifier.contains(Modifier::REVERSED));
        let text = window.take_selected(drawn.buffer);
        assert_eq!(text.as_deref(), Some("line\nsecond"));
        assert_eq!(window.take_selected(drawn.buffer), None, "answered once");

        let mut marks = |window: &mut Window| drawn_marking(&mut terminal, window);
        assert!(marks(&mut window).0, "shown until dropped");
        window.take_event(Event::Key(key(KeyCode::Char('x'))), 5);
        assert!(!marks(&mut window).0, "a key drops it");

        window.take_event(mouse(MouseEventKind::Down(left), 0, 0), 5);
        window.take_event(mouse(MouseEventKind::Up(left), 4, 0), 5);
        assert!(marks(&mut window).0, "marked again");
        window.take_event(mouse(MouseEventKind::Down(left), 2, 1), 5);
        assert!(!marks(&mut window).0, "a new press drops it, and marks nothing until dragged");
        window.take_event(mouse(MouseEventKind::Up(left), 2, 1), 5);
        assert_eq!(marks(&mut window), (false, None), "a click copies nothing");

        window.take_event(mouse(MouseEventKind::Down(left), 0, 0), 5);
        window.take_event(mouse(MouseEventKind::Drag(left), 4, 0), 5);
        assert!(marks(&mut window).0, "shown while dragged");
        window.take_event(mouse(MouseEventKind::ScrollUp, 4, 0), 5);
        assert!(!marks(&mut window).0, "the wheel drops it");
        window.take_event(mouse(MouseEventKind::Down(left), 3, 0), 5);
        window.take_event(mouse(MouseEventKind::Up(left), 3, 0), 5);
        assert!(!marks(&mut window).0, "a click marks nothing");
    }

    /// The left button pressed and released at `column` and `row` of `window`: what that answered.
    fn clicked(window: &mut Window, column: u16, row: u16) -> Option<Typed> {
        let left = MouseButton::Left;
        window.take_event(mouse(MouseEventKind::Down(left), column, row), 5);
        window.take_event(mouse(MouseEventKind::Up(left), column, row), 5)
    }

    /// A click on a choice chooses it, also while something is typed; one on the question or the
    /// keys chooses nothing, and dragging over the choices marks them.
    #[test]
    fn a_click_chooses_what_it_is_on() {
        let mut window = Window::default();
        window.ask(Some(&Asked::Shown(approval())));
        let (rows, _) = drawn(&mut window, 60, 8);
        assert_eq!(&rows[3..5], ["❯ 1. Yes", "  2. No"]);
        assert_eq!(clicked(&mut window, 4, 4), Some(Typed::Chosen(vec![1])));
        assert_eq!(clicked(&mut window, 4, 2), None, "the question");
        assert_eq!(clicked(&mut window, 4, 5), None, "the keys");
        assert_eq!(clicked(&mut window, 30, 0), None, "what was printed");
        window.take_key(&key(KeyCode::Char('x')), 5);
        assert_eq!(clicked(&mut window, 40, 3), Some(Typed::Chosen(vec![0])), "beside its words");

        let left = MouseButton::Left;
        window.take_event(mouse(MouseEventKind::Down(left), 2, 3), 5);
        window.take_event(mouse(MouseEventKind::Drag(left), 5, 4), 5);
        assert_eq!(window.take_event(mouse(MouseEventKind::Up(left), 5, 4), 5), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).expect("a test terminal");
        let marking = drawn_marking(&mut terminal, &mut window);
        assert_eq!(marking, (true, Some("1. Yes\n  2. N".to_string())), "marked, not chosen");
    }

    /// Where several choices may be taken, a click checks the choice it is on, the cursor moved
    /// to it, or takes its check away - also on the second row of a choice wrapped.
    #[test]
    fn a_click_checks_where_several_may_be_chosen() {
        let mut window = Window::default();
        window.ask(Some(&Asked::Shown(several(&["a", "a long choice that wraps"]))));
        let (rows, _) = drawn(&mut window, 30, 20);
        let long = rows.iter().position(|row| row.contains("2. a long")).expect("shown");
        let long = u16::try_from(long).expect("a row");
        assert_eq!(clicked(&mut window, 2, long + 1), None);
        let (rows, _) = drawn(&mut window, 30, 20);
        assert!(rows[usize::from(long)].starts_with("❯ [x] 2. a long"), "{rows:?}");
        assert_eq!(clicked(&mut window, 2, long - 1), None);
        assert_eq!(clicked(&mut window, 2, long), None);
        let (rows, _) = drawn(&mut window, 30, 20);
        assert!(rows[usize::from(long)].starts_with("❯ [ ] 2. a long"), "{rows:?}");
        assert_eq!(window.take_key(&key(KeyCode::Enter), 5), Some(Typed::Chosen(vec![0])));
    }

    /// A tool call stands with its result, folded - also one told of another channel, whose
    /// result comes once the user talks there no more -; a click on its head unfolds it and folds
    /// it again, Ctrl+O every call at once (issue 586).
    #[test]
    fn a_tool_call_is_unfolded_and_folded_by_a_click_or_ctrl_o() {
        let mut window = Window::default();
        let channel_id = ChannelId::new();
        let call = ToolCall { arguments: json!({ "command": "ls" }), ..make_tool_call("run") };
        let called = Printed::ToolCall {
            channel_id,
            sender: "helper".to_string(),
            call: Box::new(call.clone()),
        };
        window.print(&called);
        let result =
            ToolCallResult::of(&call, ToolResultStatus::Ok, "a\nb".to_string(), Default::default());
        let side = Name::new("side").unwrap();
        let ended = Printed::ToolResult { channel_id, result: Box::new(result) };
        let elsewhere = Printed::Elsewhere { channel: side, printed: Box::new(ended) };
        window.print(&elsewhere);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert_eq!(
            rows[..2],
            [r#"▸ helper → run {"command":"ls"}"#, "    ⎿ run answered: a (and 1 line more)"],
            "joined by its result, folded"
        );

        assert_eq!(clicked(&mut window, 3, 0), None);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert_eq!(rows[0], r#"▾ helper → run {"command":"ls"}"#, "unfolded: {rows:?}");
        assert_eq!(rows[4..6], ["      a", "      b"]);
        clicked(&mut window, 3, 0);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert!(rows[0].starts_with('▸'), "folded again: {rows:?}");

        let control = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(window.take_event(Event::Key(control), 5), None);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert!(rows[0].starts_with('▾'), "every call unfolded: {rows:?}");
        window.take_event(Event::Key(control), 5);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert!(rows[0].starts_with('▸'), "every call folded: {rows:?}");

        let left = MouseButton::Left;
        window.take_event(mouse(MouseEventKind::Down(left), 2, 0), 5);
        window.take_event(mouse(MouseEventKind::Drag(left), 9, 0), 5);
        window.take_event(mouse(MouseEventKind::Up(left), 9, 0), 5);
        let (rows, _) = drawn(&mut window, 60, 12);
        assert!(rows[0].starts_with('▸'), "dragged over, its head is marked, not unfolded");
        assert!(window.selection.is_some(), "marked");
    }
}
