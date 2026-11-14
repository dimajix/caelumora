//! The line the user types in the full-screen terminal: its text, where the cursor stands in it,
//! and the lines typed before (issue 569).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;
use unicode_width::UnicodeWidthChar;

/// The line being typed, edited by keys as a shell's is, with the lines typed before to go back
/// to.
#[derive(Debug, Default)]
pub(super) struct InputLine {
    /// What is typed, by character; a line break where one was typed with Alt+Enter.
    chars: Vec<char>,
    /// Where the cursor stands, as the number of characters before it.
    cursor: usize,
    /// The lines typed before, the last last.
    history: Vec<String>,
    /// Where in `history` the line typed is taken from while the user goes back, with what they
    /// typed before they did; `None` while they type a line of their own.
    browsing: Option<(usize, String)>,
}

impl InputLine {
    /// Takes in `key`, and answers the line typed if it ends it (Enter); keys that edit nothing
    /// are passed over.
    pub(super) fn take_key(&mut self, key: &KeyEvent) -> Option<String> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if alt => self.insert('\n'),
            KeyCode::Enter => return Some(self.take()),
            KeyCode::Char('a') if control => self.cursor = 0,
            KeyCode::Char('e') if control => self.cursor = self.chars.len(),
            KeyCode::Char('u') if control => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if control => self.chars.truncate(self.cursor),
            KeyCode::Char('w') if control => self.delete_word(),
            KeyCode::Char(c) if !control && !alt => self.insert(c),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Up => self.back(),
            KeyCode::Down => self.forth(),
            _ => {}
        }
        None
    }

    /// Inserts `text` where the cursor stands, as pasted.
    pub(super) fn paste(&mut self, text: &str) {
        for c in text.chars().filter(|c| *c != '\r') {
            self.insert(c);
        }
    }

    /// Whether nothing is typed.
    pub(super) fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The rows `prompt` and the line typed after it take at `width` columns, broken where a row
    /// is full and where a line break was typed, with where the cursor stands among them.
    pub(super) fn rows(&self, prompt: &str, width: u16) -> (Vec<String>, Position) {
        let width = usize::from(width.max(1));
        let mut rows = vec![String::new()];
        let mut column = 0;
        let mut cursor = None;
        let typed = self.chars.iter().enumerate().map(|(at, c)| (Some(at), *c));
        for (at, c) in prompt.chars().map(|c| (None, c)).chain(typed) {
            let c_width = c.width().unwrap_or(0);
            if c != '\n' && column + c_width > width {
                rows.push(String::new());
                column = 0;
            }
            if at == Some(self.cursor) {
                cursor = Some((column, rows.len() - 1));
            }
            if c == '\n' {
                rows.push(String::new());
                column = 0;
            } else if let Some(row) = rows.last_mut() {
                row.push(c);
                column += c_width;
            }
        }
        let (column, row) = cursor.unwrap_or_else(|| {
            if column >= width {
                rows.push(String::new());
                column = 0;
            }
            (column, rows.len() - 1)
        });
        (rows, Position::new(to_u16(column), to_u16(row)))
    }

    /// Inserts `c` where the cursor stands, and moves the cursor past it.
    fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Deletes the word before the cursor, with the spaces after it.
    fn delete_word(&mut self) {
        let mut start = self.cursor;
        while start > 0 && self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Ends the line typed: answers it, keeps it among those typed before unless it is empty or
    /// the last again, and begins a new one.
    fn take(&mut self) -> String {
        let line: String = self.chars.drain(..).collect();
        self.cursor = 0;
        self.browsing = None;
        if !line.trim().is_empty() && self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        line
    }

    /// Goes back to the line typed before the one shown, keeping what was typed before going back.
    fn back(&mut self) {
        let at = match &self.browsing {
            Some((0, _)) => return,
            Some((at, _)) => at - 1,
            None if self.history.is_empty() => return,
            None => {
                let draft = self.chars.iter().collect();
                self.browsing = Some((self.history.len(), draft));
                self.history.len() - 1
            }
        };
        if let Some((browsed, _)) = &mut self.browsing {
            *browsed = at;
        }
        let line = self.history[at].clone();
        self.show(&line);
    }

    /// Goes forth to the line typed after the one shown, and past the last to what was typed
    /// before going back.
    fn forth(&mut self) {
        let Some((at, draft)) = &mut self.browsing else { return };
        *at += 1;
        let line = match self.history.get(*at) {
            Some(line) => line.clone(),
            None => {
                let draft = draft.clone();
                self.browsing = None;
                draft
            }
        };
        self.show(&line);
    }

    /// Shows `line` as the line typed, the cursor at its end.
    fn show(&mut self, line: &str) {
        self.chars = line.chars().collect();
        self.cursor = self.chars.len();
    }
}

/// `value` as a column or row of the terminal, which has no more than `u16::MAX` of them.
fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn control(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(input: &mut InputLine, text: &str) {
        for c in text.chars() {
            input.take_key(&key(KeyCode::Char(c)));
        }
    }

    /// The text typed, as the rows of a window wide enough show it.
    fn text(input: &InputLine) -> String {
        input.rows("", u16::MAX).0.join("\n")
    }

    /// Keys edit the line as a shell's are, and Enter ends it.
    #[test]
    fn keys_edit_the_line_and_enter_ends_it() {
        let mut input = InputLine::default();
        typed(&mut input, "hello world");
        input.take_key(&control('w'));
        assert_eq!(text(&input), "hello ");
        input.take_key(&control('a'));
        typed(&mut input, ">");
        input.take_key(&key(KeyCode::Delete));
        assert_eq!(text(&input), ">ello ");
        input.take_key(&control('e'));
        input.take_key(&key(KeyCode::Backspace));
        input.take_key(&key(KeyCode::Left));
        input.take_key(&control('k'));
        assert_eq!(text(&input), ">ell");
        input.take_key(&control('u'));
        assert!(input.is_empty());
        typed(&mut input, "ü");
        input.paste("ber\r\nall");
        assert_eq!(input.take_key(&key(KeyCode::Enter)).as_deref(), Some("über\nall"));
        assert!(input.is_empty(), "a new line begun");
    }

    /// Up and Down go through the lines typed before, and back to what was being typed.
    #[test]
    fn the_lines_typed_before_are_gone_through() {
        let mut input = InputLine::default();
        for line in ["one", "two", "two"] {
            typed(&mut input, line);
            input.take_key(&key(KeyCode::Enter));
        }
        typed(&mut input, "dra");
        input.take_key(&key(KeyCode::Up));
        assert_eq!(text(&input), "two");
        input.take_key(&key(KeyCode::Up));
        assert_eq!(text(&input), "one", "the same line kept once");
        input.take_key(&key(KeyCode::Up));
        assert_eq!(text(&input), "one", "the first");
        input.take_key(&key(KeyCode::Down));
        input.take_key(&key(KeyCode::Down));
        assert_eq!(text(&input), "dra", "what was being typed");
        input.take_key(&key(KeyCode::Down));
        assert_eq!(text(&input), "dra");
    }

    /// The prompt and the line take rows of the width given, broken where a row is full or a line
    /// break was typed, wide characters counted by their width; the cursor stands where it is,
    /// also on a character that begins a row.
    #[test]
    fn the_line_takes_rows_of_the_width_with_the_cursor_where_it_stands() {
        let mut input = InputLine::default();
        assert_eq!(input.rows(">> ", 10), (vec![">> ".to_string()], Position::new(3, 0)));
        typed(&mut input, "abcdefg");
        let rows = vec![">> abcdefg".to_string(), String::new()];
        assert_eq!(input.rows(">> ", 10), (rows, Position::new(0, 1)), "at the end, a row full");
        input.take_key(&key(KeyCode::Home));
        assert_eq!(input.rows(">> ", 10).1, Position::new(3, 0));
        input.take_key(&key(KeyCode::End));
        typed(&mut input, "h");
        input.take_key(&key(KeyCode::Left));
        assert_eq!(
            input.rows(">> ", 10),
            (vec![">> abcdefg".to_string(), "h".to_string()], Position::new(0, 1)),
            "on the h, which begins a row"
        );
        let mut wide = InputLine::default();
        typed(&mut wide, "日本語");
        let (rows, cursor) = wide.rows(">> ", 8);
        assert_eq!(rows, [">> 日本", "語"]);
        assert_eq!(cursor, Position::new(2, 1));
        wide.paste("\nx");
        assert_eq!(wide.rows(">> ", 8).0, [">> 日本", "語", "x"]);
    }
}
