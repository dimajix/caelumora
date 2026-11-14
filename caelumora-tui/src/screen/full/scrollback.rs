//! What the full-screen terminal printed, and what streams after it, as the lines its upper part
//! scrolls through (issue 569).

use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

/// The lines printed and, below them, the live lines of what streams, with how far the user
/// scrolled up: following the end where they did not, staying where they are where they did - as
/// lines are printed, and as what streams grows or is replaced by what was printed of it.
#[derive(Debug, Default)]
pub(super) struct Scrollback {
    /// The lines printed, in order.
    lines: Vec<Line<'static>>,
    /// What streams after them, replaced as a whole as it changes.
    live: Vec<Line<'static>>,
    /// How many rows above the end of the lines printed the user scrolled the bottom of what is
    /// seen; `None` while the end is followed, the live lines included.
    up: Option<usize>,
    /// The width the lines were last drawn at, by which rows are counted between two drawings.
    width: u16,
}

impl Scrollback {
    /// Adds `lines` to those printed; where the user scrolled up, what they see stays.
    pub(super) fn push(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        let start = self.lines.len();
        self.lines.extend(lines);
        let added: usize = self.lines[start..].iter().map(|line| rows(line, self.width)).sum();
        if let Some(up) = &mut self.up {
            *up += added;
        }
    }

    /// Replaces what streams after the lines printed by `live`; what the user sees where they
    /// scrolled up stays.
    pub(super) fn set_live(&mut self, live: Vec<Line<'static>>) {
        self.live = live;
    }

    /// Scrolls up by `by` rows, as far as the first line - from the end past what streams, which
    /// then moves what is seen no more as it grows.
    pub(super) fn scroll_up(&mut self, by: usize) {
        let up = match self.up {
            Some(up) => up.saturating_add(by),
            None => by.saturating_sub(self.live_rows()),
        };
        self.up = Some(up);
    }

    /// Scrolls down by `by` rows; at the end of the lines printed, the end is followed again.
    pub(super) fn scroll_down(&mut self, by: usize) {
        self.up = match self.up {
            Some(up) if up > by => Some(up - by),
            _ => None,
        };
    }

    /// Follows the end again.
    pub(super) fn follow_end(&mut self) {
        self.up = None;
    }

    /// How many rows lie below what is seen; none while the end is followed.
    pub(super) fn below(&self) -> usize {
        self.up.map_or(0, |up| up + self.live_rows())
    }

    /// What is seen in `height` rows of `width` columns: the lines that fill them, as far up as
    /// the user scrolled, with how many rows of the first of them lie above the top. Where the
    /// user scrolled past the first line, they are taken back to it; where all that was printed
    /// fits, the end is followed.
    pub(super) fn visible(&mut self, width: u16, height: u16) -> (Vec<Line<'static>>, u16) {
        self.width = width;
        let high = usize::from(height);
        let live: &[Line<'static>] = if self.up.is_none() { &self.live } else { &[] };
        let wanted = high + self.up.unwrap_or(0);
        let mut seen = Vec::new();
        let mut filled = 0;
        for line in live.iter().rev().chain(self.lines.iter().rev()) {
            if filled >= wanted {
                break;
            }
            filled += rows(line, width);
            seen.push(line.clone());
        }
        seen.reverse();
        if filled < wanted && self.up.is_some() {
            // Scrolled past the first line: taken back to it - where all that was printed fits,
            // to the end, which is followed again.
            let up = filled.saturating_sub(high);
            if up == 0 {
                self.up = None;
                return self.visible(width, height);
            }
            self.up = Some(up);
        }
        let above = filled.saturating_sub(high + self.up.unwrap_or(0));
        (seen, u16::try_from(above).unwrap_or(u16::MAX))
    }

    /// How many rows the live lines take at the width last drawn.
    fn live_rows(&self) -> usize {
        self.live.iter().map(|line| rows(line, self.width)).sum()
    }
}

/// How many rows `line` takes at `width` columns, broken at words as it is drawn - which
/// ratatui's `Paragraph::line_count` counts, under its feature `unstable-rendered-line-info`.
fn rows(line: &Line<'static>, width: u16) -> usize {
    Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<Line<'static>> {
        texts.iter().map(|text| Line::from(text.to_string())).collect()
    }

    /// What is drawn in `height` rows of lines of one row each, as `visible` answered them.
    fn drawn((seen, above): (Vec<Line<'static>>, u16), height: usize) -> Vec<String> {
        seen.iter().skip(usize::from(above)).take(height).map(|line| line.to_string()).collect()
    }

    /// The end is followed, the live lines after those printed; scrolled up, what is seen stays as
    /// lines are printed, and scrolling down to the end follows it again.
    #[test]
    fn the_end_is_followed_unless_scrolled_up() {
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b", "c", "d"]));
        scrollback.set_live(lines(&["live"]));
        assert_eq!(drawn(scrollback.visible(20, 2), 2), ["d", "live"]);
        scrollback.set_live(Vec::new());

        scrollback.scroll_up(2);
        assert_eq!(drawn(scrollback.visible(20, 2), 2), ["a", "b"]);
        assert_eq!(scrollback.below(), 2);
        scrollback.push(lines(&["e"]));
        assert_eq!(drawn(scrollback.visible(20, 2), 2), ["a", "b"], "what is seen stays");

        scrollback.scroll_down(10);
        assert_eq!(scrollback.below(), 0, "the end followed");
        assert_eq!(drawn(scrollback.visible(20, 2), 2), ["d", "e"]);
    }

    /// Scrolled up, what is seen stays as what streams grows, and as it is replaced by what was
    /// printed of it.
    #[test]
    fn what_streams_does_not_move_what_is_seen() {
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b", "c", "d"]));
        scrollback.set_live(lines(&["one"]));
        scrollback.visible(20, 2);
        scrollback.scroll_up(3);
        let seen = drawn(scrollback.visible(20, 2), 2);
        assert_eq!(seen, ["a", "b"]);
        scrollback.set_live(lines(&["one", "two", "three"]));
        assert_eq!(drawn(scrollback.visible(20, 2), 2), seen, "it grew");
        scrollback.set_live(Vec::new());
        scrollback.push(lines(&["one two three"]));
        assert_eq!(drawn(scrollback.visible(20, 2), 2), seen, "replaced by what was printed");
    }

    /// Scrolled past the first line, the user is taken back to it.
    #[test]
    fn scrolling_stops_at_the_first_line() {
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b", "c"]));
        scrollback.scroll_up(100);
        assert_eq!(drawn(scrollback.visible(20, 2), 2), ["a", "b"]);
        assert_eq!(scrollback.below(), 1);
    }

    /// Where all that was printed fits, there is nothing to scroll: the end is followed, with
    /// what streams.
    #[test]
    fn what_fits_is_not_scrolled() {
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a"]));
        scrollback.set_live(lines(&["live"]));
        scrollback.scroll_up(5);
        assert_eq!(drawn(scrollback.visible(20, 3), 3), ["a", "live"]);
        assert_eq!(scrollback.below(), 0);
    }

    /// A line broken into rows counts by its rows, and is seen in part where it does not fit.
    #[test]
    fn a_long_line_counts_by_its_rows() {
        let long = "one two three four";
        assert_eq!(rows(&Line::from(long), 14), 2, "broken at a word");
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&[long, "end"]));
        let (seen, above) = scrollback.visible(14, 2);
        assert_eq!(seen.len(), 2);
        assert_eq!(above, 1, "its first row above the top");
    }
}
