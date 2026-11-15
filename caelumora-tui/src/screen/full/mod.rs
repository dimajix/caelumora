//! The way of drawing `full`: the terminal's whole window, what was printed scrolled above and the
//! line typed below, as Claude Code draws it (issue 569).

mod choosing;
mod clipboard;
mod input;
mod scrollback;
mod selection;
mod styled;

use super::{Screen, Typed};
use crate::asking::Asked;
use crate::printed::Printed;
use crate::under_way::UnderWay;
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
use std::time::{Duration, Instant};
use styled::{live_lines, status_line, styled, typed_line};
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
/// printed and what streams, scrolled by the user alone; what is asked of the user; the status
/// line of the answers under way; the line typed; and what the user marks with the mouse, and the
/// choice they click.
#[derive(Debug, Default)]
struct Window {
    /// What was printed, and what streams after it.
    scrollback: Scrollback,
    /// The answers under way in the channel talked in.
    under_way: Vec<UnderWay>,
    /// What is asked of the user, as it is chosen among; told on the rule only, where it is put
    /// aside.
    choosing: Option<Choosing>,
    /// Where what is asked was drawn last, which a click there chooses among.
    asked_area: Rect,
    /// The line being typed.
    input: InputLine,
    /// What the user marks with the mouse, until a key or the left button is pressed, text pasted,
    /// the wheel turned or the window resized - each of which may move other text under its cells.
    selection: Option<Selection>,
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
        self.window.scrollback.push(printed.iter().flat_map(styled));
        self.changed = true;
    }

    fn show_under_way(&mut self, under_way: &[UnderWay]) {
        self.window.show_under_way(under_way);
        self.changed = true;
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
                        if let Some(typed) = self.window.take_event(event, usize::from(height / 2)) {
                            self.draw();
                            return typed;
                        }
                    }
                    Some(Err(error)) => return Typed::Failed(error.to_string()),
                    None => return Typed::Ended,
                },
                _ = self.frames.tick() => {
                    // The status line tells for how long an answer is under way.
                    self.changed |= !self.window.under_way.is_empty();
                }
            }
        }
    }
}

impl Window {
    /// Shows the answers `under_way`: what they streamed after what was printed, and where they
    /// stand in the status line.
    fn show_under_way(&mut self, under_way: &[UnderWay]) {
        self.scrollback.set_live(live_lines(under_way));
        self.under_way = under_way.to_vec();
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
    /// is dragged over, and a click with it chooses what it is on (`Window::take_click`) - answered
    /// where that ends what is asked.
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

    /// Takes in a click at `at`: one on a choice of what is asked chooses it
    /// (`Choosing::take_click`) - answered where that ends what is asked. Unlike a digit, which is
    /// typed where something is, a click chooses also then: it is no text.
    fn take_click(&mut self, at: Position) -> Option<Typed> {
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
    /// chooses among what is asked while nothing is typed (`Choosing::take_key`), or edits the
    /// line typed - and answers the line where it ends it, shown
    /// as sent and the end of what was printed followed.
    fn take_key(&mut self, key: &KeyEvent, page: usize) -> Option<Typed> {
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
            _ => {
                let line = self.input.take_key(key)?;
                self.scrollback.push(typed_line(&line));
                self.scrollback.follow_end();
                return Some(Typed::Line(line));
            }
        }
        None
    }

    /// Draws the window on `frame` at `now`: what was printed and what streams; the status line
    /// where answers are under way; a rule, which tells where the user scrolled up; the line
    /// typed, with the cursor in it; and over all, what the user marks.
    fn draw(&mut self, frame: &mut Frame, now: Instant) {
        let area = frame.area();
        let (rows, cursor) = self.input.rows(PROMPT, area.width);
        let input_rows = u16::try_from(rows.len()).unwrap_or(u16::MAX).min(INPUT_ROWS);
        let status_rows = u16::from(!self.under_way.is_empty());
        let asked = self.choosing.as_ref().map(Choosing::rows).filter(|rows| !rows.is_empty());
        let asked = asked.map(|rows| Paragraph::new(rows).wrap(Wrap { trim: false }));
        let asked_rows = asked.as_ref().map_or(0, |asked| asked.line_count(area.width));
        let asked_rows = u16::try_from(asked_rows).unwrap_or(u16::MAX).min(area.height / 2);
        let [printed, asking, status, rule, typed] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(asked_rows),
            Constraint::Length(status_rows),
            Constraint::Length(1),
            Constraint::Length(input_rows),
        ])
        .areas(area);
        self.asked_area = asking;

        let (seen, above) = self.scrollback.visible(printed.width, printed.height);
        let seen = Paragraph::new(seen).wrap(Wrap { trim: false }).scroll((above, 0));
        frame.render_widget(seen, printed);
        if let Some(asked) = asked {
            frame.render_widget(asked, asking);
        }
        frame.render_widget(status_line(&self.under_way, now), status);
        let waits = self.choosing.as_ref().and_then(Choosing::waiting);
        frame.render_widget(rule_line(self.scrollback.below(), waits, rule), rule);

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
    use crate::asking::Asking;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::approval::ApprovalRequest;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::{Choice, Question, QuestionRequest, Questions};
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;
    use caelumora_domain::work::TurnId;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

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

    /// The window shows what was printed above, the status line of an answer under way, the rule,
    /// and the line typed with the cursor after it.
    #[test]
    fn the_window_shows_what_was_printed_the_status_and_the_line_typed() {
        let mut window = Window::default();
        window.scrollback.push(styled(&Printed::Notice("Talking in main now".to_string())));
        let agent = ActorId::Agent(AgentId::new());
        let answer = UnderWay::began(agent, "helper".to_string(), TurnId::new(), None);
        window.show_under_way(&[answer]);
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

    /// A line typed is answered as it ends, and shown as sent; scrolled up, the rule tells how
    /// far, and sending a line follows the end again.
    #[test]
    fn a_line_sent_is_shown_and_follows_the_end() {
        let mut window = Window::default();
        let printed = (0..10).map(|n| Printed::Plain(format!("line {n}")));
        window.scrollback.push(printed.flat_map(|printed| styled(&printed)));
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
        Asking::Approval(ApprovalRequest { description: "Run ls".to_string(), ..request })
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

    /// Ctrl+C ends the program, Ctrl+D the input where nothing is typed.
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
        window.scrollback.push(printed.iter().flat_map(styled));
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
}
