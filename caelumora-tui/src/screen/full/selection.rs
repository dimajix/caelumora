//! What the user marks with the mouse in the full-screen terminal, as a terminal marks it (issue
//! 576).
//!
//! The marking holds cells of the window, not text: what it shows and copies is what the window
//! drew there.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

/// What the user marks with the mouse: the cells from where the left button was pressed to where
/// it is dragged or was released, in reading order, row by row as a terminal marks them.
#[derive(Debug)]
pub(super) struct Selection {
    /// The cell the left button was pressed on.
    from: Position,
    /// The cell the button is dragged to, or was released on.
    to: Position,
    /// Where the marking stands.
    state: Marking,
}

/// Where the marking stands: being dragged, released and not yet copied, or copied.
#[derive(Debug, PartialEq, Eq)]
enum Marking {
    Dragging,
    Released,
    Copied,
}

impl Selection {
    /// Begins marking at the cell `at`, where the left button was pressed.
    pub(super) fn new(at: Position) -> Self {
        Self { from: at, to: at, state: Marking::Dragging }
    }

    /// Marks up to the cell `at`, where the button is dragged.
    pub(super) fn drag_to(&mut self, at: Position) {
        self.to = at;
    }

    /// Ends marking at `at`; answers whether anything is marked, `false` for a click - the button
    /// released on the cell it was pressed on.
    pub(super) fn release(&mut self, at: Position) -> bool {
        self.to = at;
        self.state = Marking::Released;
        self.from != self.to
    }

    /// Shows the cells marked on `buffer`, reversed - none while the button is still on the cell
    /// it was pressed on, as a terminal shows none for a click.
    pub(super) fn highlight(&self, buffer: &mut Buffer) {
        if self.from == self.to {
            return;
        }
        for (y, columns) in self.rows(buffer.area) {
            for x in columns {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(Style::new().reversed());
                }
            }
        }
    }

    /// Once released, the text of the cells marked as `buffer` shows them, rows broken by line
    /// breaks and blanks at their ends dropped; answered once.
    pub(super) fn take_released(&mut self, buffer: &Buffer) -> Option<String> {
        if self.state != Marking::Released {
            return None;
        }
        self.state = Marking::Copied;
        let rows: Vec<String> =
            self.rows(buffer.area).map(|(y, columns)| row_text(buffer, y, columns)).collect();
        Some(rows.join("\n"))
    }

    /// The rows marked within `area`, each with the columns marked in it: from the first cell
    /// to the end of its row, whole rows between, and the last row up to the last cell.
    fn rows(&self, area: Rect) -> impl Iterator<Item = (u16, Range<u16>)> {
        let (first, last) = match (self.from.y, self.from.x) <= (self.to.y, self.to.x) {
            true => (self.from, self.to),
            false => (self.to, self.from),
        };
        let within = area.top()..area.bottom();
        (first.y..=last.y).filter(move |y| within.contains(y)).map(move |y| {
            let start = if y == first.y { first.x } else { area.left() };
            let end = if y == last.y { last.x.saturating_add(1) } else { area.right() };
            (y, start.max(area.left())..end.min(area.right()))
        })
    }
}

/// The text `buffer` shows in the `columns` of row `y`, without the cells a wide character before
/// them covers, and without the blanks at its end.
fn row_text(buffer: &Buffer, y: u16, columns: Range<u16>) -> String {
    let mut text = String::new();
    let mut covered = 0;
    for x in columns {
        let symbol = buffer.cell((x, y)).map_or(" ", |cell| cell.symbol());
        if covered > 0 {
            covered -= 1;
            continue;
        }
        text.push_str(symbol);
        covered = symbol.width().saturating_sub(1);
    }
    text.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    /// Marks from `from` to `to` and releases there.
    fn marked(from: (u16, u16), to: (u16, u16)) -> Selection {
        let mut selection = Selection::new(from.into());
        selection.drag_to(to.into());
        selection.release(to.into());
        selection
    }

    /// From the first cell to the end of its row, whole rows between, and the last row up to the
    /// last cell - also marked from below upwards; blanks at the ends of rows are dropped.
    #[test]
    fn rows_are_marked_as_a_terminal_marks_them() {
        let buffer = Buffer::with_lines(["first row  ", "second", "third row"]);
        let text = "row\nsecond\nthird";
        assert_eq!(marked((6, 0), (4, 2)).take_released(&buffer).as_deref(), Some(text));
        assert_eq!(marked((4, 2), (6, 0)).take_released(&buffer).as_deref(), Some(text));
        assert_eq!(marked((1, 1), (3, 1)).take_released(&buffer).as_deref(), Some("eco"));
    }

    /// The text is answered once released, and once only.
    #[test]
    fn the_text_is_answered_once_released() {
        let buffer = Buffer::with_lines(["some text"]);
        let mut selection = Selection::new(Position::new(0, 0));
        selection.drag_to(Position::new(3, 0));
        assert_eq!(selection.take_released(&buffer), None, "still dragged");
        assert!(selection.release(Position::new(3, 0)));
        assert_eq!(selection.take_released(&buffer).as_deref(), Some("some"));
        assert_eq!(selection.take_released(&buffer), None, "already copied");
    }

    /// A click marks nothing, and shows nothing while the button is held.
    #[test]
    fn a_click_marks_nothing() {
        let mut buffer = Buffer::with_lines(["abc"]);
        let mut selection = Selection::new(Position::new(2, 0));
        selection.highlight(&mut buffer);
        assert!(!buffer.content.iter().any(|cell| cell.modifier.contains(Modifier::REVERSED)));
        assert!(!selection.release(Position::new(2, 0)));
    }

    /// A wide character is copied once, not with the cell it covers.
    #[test]
    fn a_wide_character_is_copied_once() {
        let buffer = Buffer::with_lines(["a日本b"]);
        assert_eq!(marked((0, 0), (5, 0)).take_released(&buffer).as_deref(), Some("a日本b"));
    }

    /// The cells marked are shown reversed, those around them not; a marking beyond the window,
    /// which shrank, is shown within it.
    #[test]
    fn the_cells_marked_are_shown_reversed() {
        let mut buffer = Buffer::with_lines(["abc", "def"]);
        marked((1, 0), (9, 9)).highlight(&mut buffer);
        let reversed = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::REVERSED);
        assert!(!reversed(0, 0));
        assert!(reversed(1, 0) && reversed(2, 0) && reversed(0, 1) && reversed(2, 1));
    }
}
