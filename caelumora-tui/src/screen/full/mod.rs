//! The way of drawing `full`: the terminal's whole window, what was printed scrolled above and the
//! line typed below, as Claude Code draws it (issue 569).

mod input;
mod scrollback;
mod styled;

use super::{Screen, Typed};
use crate::printed::Printed;
use crate::under_way::UnderWay;
use async_trait::async_trait;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use input::InputLine;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use scrollback::Scrollback;
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

/// The way of drawing `full`: the terminal taken over until it is dropped, its window drawn as
/// `Window` stands, at most once a frame.
pub(crate) struct FullScreen {
    /// The terminal drawn on, in its alternate screen.
    terminal: DefaultTerminal,
    /// What the terminal tells of the keys pressed, the mouse and the window.
    events: EventStream,
    /// What the window shows.
    window: Window,
    /// What wakes the window to be drawn anew while answers are under way, and what was drawn too
    /// early to be drawn at once.
    frames: Interval,
    /// Whether something changed since the window was drawn.
    changed: bool,
    /// When the window was drawn last.
    drawn: Instant,
}

/// What the full-screen window shows, and how keys, the mouse and pasted text change it: what was
/// printed and what streams, scrolled by the user alone; the status line of the answers under
/// way; and the line typed.
#[derive(Debug, Default)]
struct Window {
    /// What was printed, and what streams after it.
    scrollback: Scrollback,
    /// The answers under way in the channel talked in.
    under_way: Vec<UnderWay>,
    /// The line being typed.
    input: InputLine,
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
            frames,
            changed: true,
            drawn: Instant::now(),
        })
    }

    /// Draws the window, if something changed since it was drawn and it was not drawn too short a
    /// time ago - then the next frame does.
    fn draw(&mut self) {
        if !self.changed || self.drawn.elapsed() < FRAME {
            return;
        }
        let window = &mut self.window;
        if let Err(error) = self.terminal.draw(|frame| window.draw(frame, Instant::now())) {
            warn!("The terminal could not draw its window: {error}");
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

    /// Takes in `event`, a page being `page` rows, and answers what the user typed if it ends a
    /// line or the input.
    fn take_event(&mut self, event: Event, page: usize) -> Option<Typed> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.take_key(&key, page),
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.scrollback.scroll_up(WHEEL_ROWS),
                    MouseEventKind::ScrollDown => self.scrollback.scroll_down(WHEEL_ROWS),
                    _ => {}
                }
                None
            }
            Event::Paste(text) => {
                self.input.paste(&text);
                None
            }
            _ => None,
        }
    }

    /// Takes in `key`, a page being `page` rows: what ends the program or the input, scrolls, or
    /// edits the line typed - and answers the line where it ends it, shown as sent and the end of
    /// what was printed followed.
    fn take_key(&mut self, key: &KeyEvent, page: usize) -> Option<Typed> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
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
    /// where answers are under way; a rule, which tells where the user scrolled up; and the line
    /// typed, with the cursor in it.
    fn draw(&mut self, frame: &mut Frame, now: Instant) {
        let area = frame.area();
        let (rows, cursor) = self.input.rows(PROMPT, area.width);
        let input_rows = u16::try_from(rows.len()).unwrap_or(u16::MAX).min(INPUT_ROWS);
        let status_rows = u16::from(!self.under_way.is_empty());
        let [printed, status, rule, typed] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(status_rows),
            Constraint::Length(1),
            Constraint::Length(input_rows),
        ])
        .areas(area);

        let (seen, above) = self.scrollback.visible(printed.width, printed.height);
        let seen = Paragraph::new(seen).wrap(Wrap { trim: false }).scroll((above, 0));
        frame.render_widget(seen, printed);
        frame.render_widget(status_line(&self.under_way, now), status);
        frame.render_widget(rule_line(self.scrollback.below(), rule), rule);

        // The rows about the cursor, where the line takes more than are shown.
        let first = cursor.y.saturating_sub(input_rows.saturating_sub(1));
        let shown: Vec<Line> = rows.into_iter().skip(usize::from(first)).map(Line::from).collect();
        frame.render_widget(Paragraph::new(shown), typed);
        frame.set_cursor_position(Position::new(typed.x + cursor.x, typed.y + cursor.y - first));
    }
}

/// The rule between what was printed and the line typed, across `area`: how many rows lie
/// `below` where the user scrolled up, and how to follow the end again.
fn rule_line(below: usize, area: Rect) -> Line<'static> {
    let told = match below {
        0 => String::new(),
        below => format!(" ↓ {below} rows below · Ctrl+End follows the end "),
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
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::work::TurnId;
    use ratatui::backend::TestBackend;
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
}
