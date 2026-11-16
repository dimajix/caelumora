//! What the full-screen terminal printed, and what streams after it, as the lines its upper part
//! scrolls through (issue 569): what was printed as lines, and the tool calls among it as entries
//! of their own, which their results join and which fold and unfold (issue 586).

use super::call::ShownCall;
use super::wrapped_rows;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::tool::ToolCallResult;
use ratatui::text::Line;

/// What was printed and, below it, the live lines of what streams, with how far the user scrolled
/// up: following the end where they did not, staying where they are where they did - as lines are
/// printed, as what streams grows or is replaced by what was printed of it, and as a call below
/// what is seen grows or shrinks.
#[derive(Debug, Default)]
pub(super) struct Scrollback {
    /// What was printed, in order.
    entries: Vec<Entry>,
    /// What streams after it, replaced as a whole as it changes.
    live: Vec<Line<'static>>,
    /// How many rows above the end of what was printed the user scrolled the bottom of what is
    /// seen; `None` while the end is followed, the live lines included.
    up: Option<usize>,
    /// The width the lines were last drawn at, by which rows are counted between two drawings.
    width: u16,
    /// The lines last seen, from the top, each with how many rows it took and - where it is the
    /// head of a call - the call's entry: what a click falls on.
    seen: Vec<(usize, Option<usize>)>,
    /// How many rows of the first line seen lay above the top.
    above: usize,
}

/// What stays where it is as a call grows or shrinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stays {
    /// What is seen at the bottom, where the call lies below it.
    Seen,
    /// The call's head - but where the end is followed and what streams takes more rows than the
    /// call grows by, or the call shrinks while the end is followed: the end stays followed then.
    Head,
}

/// One thing printed, as the scrollback keeps it.
#[derive(Debug)]
enum Entry {
    /// Lines, as they were drawn.
    Lines(Vec<Line<'static>>),
    /// A tool call, with its result once it came.
    Call(Box<ShownCall>),
}

impl Entry {
    /// The lines it is drawn as.
    fn lines(&self) -> &[Line<'static>] {
        match self {
            Entry::Lines(lines) => lines,
            Entry::Call(call) => call.lines(),
        }
    }
}

impl Scrollback {
    /// Adds `lines` to what was printed; where the user scrolled up, what they see stays.
    pub(super) fn push(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        let lines: Vec<Line<'static>> = lines.into_iter().collect();
        self.scrolled_by(self.rows(&lines));
        match self.entries.last_mut() {
            Some(Entry::Lines(printed)) => printed.extend(lines),
            _ => self.entries.push(Entry::Lines(lines)),
        }
    }

    /// Adds the tool call `call` to what was printed; where the user scrolled up, what they see
    /// stays.
    pub(super) fn push_call(&mut self, call: ShownCall) {
        self.scrolled_by(self.rows(call.lines()));
        self.entries.push(Entry::Call(Box::new(call)));
    }

    /// Joins `result`, of the channel `channel_id`, to the newest call it ends
    /// (`ShownCall::is_ended_by`) - one whose id a backend repeats ended already -; answers
    /// whether there was one. Where the call lies below what is seen, what is seen stays.
    pub(super) fn end_call(&mut self, channel_id: ChannelId, result: &ToolCallResult) -> bool {
        let found = self.entries.iter().rposition(
            |entry| matches!(entry, Entry::Call(call) if call.is_ended_by(channel_id, result)),
        );
        let Some(index) = found else { return false };
        self.change(index, Stays::Seen, |call| {
            call.end(result.clone());
            true
        });
        true
    }

    /// Unfolds the call whose head was seen at the row `row` of what is seen, or folds it; its
    /// head stays where it is (`Stays::Head`).
    pub(super) fn unfold_at(&mut self, row: u16) {
        let Some(index) = self.head_at(usize::from(row)) else { return };
        let Some(Entry::Call(call)) = self.entries.get(index) else { return };
        let unfolded = !call.is_unfolded();
        self.change(index, Stays::Head, |call| call.unfold(unfolded));
    }

    /// Unfolds every call - folds them all where none is left to unfold -; what is seen at the
    /// bottom stays where the calls lie below it.
    pub(super) fn unfold_all(&mut self) {
        let calls = || {
            self.entries.iter().filter_map(|entry| match entry {
                Entry::Call(call) => Some(call),
                Entry::Lines(_) => None,
            })
        };
        let unfolded = calls().any(|call| call.folds() && !call.is_unfolded());
        for index in 0..self.entries.len() {
            self.change(index, Stays::Seen, |call| call.unfold(unfolded));
        }
    }

    /// Replaces what streams after what was printed by `live`; what the user sees where they
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

    /// Scrolls down by `by` rows; at the end of what was printed, the end is followed again.
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
        let printed = self.entries.iter().enumerate().rev().flat_map(|(index, entry)| {
            let call = matches!(entry, Entry::Call(_));
            let head = move |at: usize| (call && at == 0).then_some(index);
            entry.lines().iter().enumerate().rev().map(move |(at, line)| (line, head(at)))
        });
        let mut seen = Vec::new();
        let mut filled = 0;
        for (line, head) in live.iter().rev().map(|line| (line, None)).chain(printed) {
            if filled >= wanted {
                break;
            }
            let rows = wrapped_rows(line, width);
            filled += rows;
            seen.push((line.clone(), rows, head));
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
        self.above = filled.saturating_sub(high + self.up.unwrap_or(0));
        self.seen = seen.iter().map(|(_, rows, head)| (*rows, *head)).collect();
        let lines = seen.into_iter().map(|(line, _, _)| line).collect();
        (lines, u16::try_from(self.above).unwrap_or(u16::MAX))
    }

    /// The entry of the call whose head was seen at the row `row` of what is seen; `None` where
    /// none was.
    fn head_at(&self, row: usize) -> Option<usize> {
        let mut top = 0;
        let row = row + self.above;
        for (rows, head) in &self.seen {
            top += rows;
            if row < top {
                return *head;
            }
        }
        None
    }

    /// Changes the call of the entry `index` by `change`, which answers whether it changed what
    /// the call shows; where it did, what `stays` stays where it is.
    fn change(&mut self, index: usize, stays: Stays, change: impl FnOnce(&mut ShownCall) -> bool) {
        let width = self.width;
        let Some(Entry::Call(call)) = self.entries.get_mut(index) else { return };
        let before = rows(call.lines(), width);
        if !change(call) {
            return;
        }
        let after = rows(call.lines(), width);
        let below: usize =
            self.entries[index + 1..].iter().map(|entry| rows(entry.lines(), width)).sum();
        self.up = match (self.up, stays) {
            (Some(up), Stays::Head) => Some((up + after).saturating_sub(before)),
            (Some(up), Stays::Seen) if below <= up => Some((up + after).saturating_sub(before)),
            (None, Stays::Head) if after > before => (after - before).checked_sub(self.live_rows()),
            (up, _) => up,
        };
        if self.up == Some(0) && self.live.is_empty() {
            // Nothing below what is seen: the end is followed again.
            self.up = None;
        }
    }

    /// Where the user scrolled up, keeps what they see as `added` rows are printed below it.
    fn scrolled_by(&mut self, added: usize) {
        if let Some(up) = &mut self.up {
            *up += added;
        }
    }

    /// How many rows `lines` take at the width last drawn.
    fn rows(&self, lines: &[Line<'static>]) -> usize {
        rows(lines, self.width)
    }

    /// How many rows the live lines take at the width last drawn.
    fn live_rows(&self) -> usize {
        self.rows(&self.live)
    }
}

/// How many rows `lines` take at `width` columns.
fn rows(lines: &[Line<'static>], width: u16) -> usize {
    lines.iter().map(|line| wrapped_rows(line, width)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolResultStatus};
    use serde_json::json;

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
        assert_eq!(wrapped_rows(&Line::from(long), 14), 2, "broken at a word");
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&[long, "end"]));
        let (seen, above) = scrollback.visible(14, 2);
        assert_eq!(seen.len(), 2);
        assert_eq!(above, 1, "its first row above the top");
    }

    /// A call of `f` in the channel `channel_id`, called by `helper`, and how it ended, answering
    /// two lines.
    fn call_and_result(channel_id: ChannelId) -> (ShownCall, ToolCallResult) {
        let call = ToolCall { arguments: json!({ "x": 1 }), ..make_tool_call("f") };
        let result = ToolCallResult::of(
            &call,
            ToolResultStatus::Ok,
            "r1\nr2".to_string(),
            Default::default(),
        );
        (ShownCall::new(channel_id, None, "helper".to_string(), call), result)
    }

    /// Scrolled up, what is seen stays as a call below it is joined by its result and unfolded;
    /// a result whose call was not shown joins nothing.
    #[test]
    fn a_call_below_what_is_seen_moves_nothing() {
        let channel_id = ChannelId::new();
        let (call, result) = call_and_result(channel_id);
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b", "c", "d"]));
        scrollback.push_call(call);
        scrollback.visible(40, 2);
        scrollback.scroll_up(3);
        let seen = drawn(scrollback.visible(40, 2), 2);
        assert!(!scrollback.end_call(ChannelId::new(), &result), "another channel's");
        assert!(scrollback.end_call(channel_id, &result));
        assert_eq!(drawn(scrollback.visible(40, 2), 2), seen, "joined by its result");
        scrollback.unfold_all();
        assert_eq!(drawn(scrollback.visible(40, 2), 2), seen, "unfolded");
        assert!(!scrollback.end_call(channel_id, &result), "ended once");
    }

    /// A call unfolded and folded by a click on its head keeps its head where it is, and the end
    /// is followed again once it is folded; a click elsewhere unfolds nothing.
    #[test]
    fn a_call_unfolded_by_its_head_keeps_it_in_place() {
        let channel_id = ChannelId::new();
        let (call, result) = call_and_result(channel_id);
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b", "c"]));
        scrollback.push_call(call);
        scrollback.end_call(channel_id, &result);
        let folded = drawn(scrollback.visible(40, 3), 3);
        assert_eq!(folded[1], r#"▸ helper → f {"x":1}"#, "{folded:?}");
        scrollback.unfold_at(0);
        scrollback.unfold_at(2);
        assert_eq!(drawn(scrollback.visible(40, 3), 3), folded, "no head clicked");
        scrollback.unfold_at(1);
        let unfolded = drawn(scrollback.visible(40, 3), 3);
        assert_eq!(unfolded[..2], ["c", r#"▾ helper → f {"x":1}"#], "the head in place");
        assert_eq!(unfolded[2], "      {");
        scrollback.unfold_at(1);
        assert_eq!(drawn(scrollback.visible(40, 3), 3), folded, "folded again");
        assert_eq!(scrollback.below(), 0, "the end followed again");
        scrollback.push(lines(&["d"]));
        assert_eq!(drawn(scrollback.visible(40, 3), 3).last().map(String::as_str), Some("d"));
    }

    /// Scrolled up, a call unfolded by its head keeps it in place too.
    #[test]
    fn scrolled_up_a_call_unfolded_keeps_its_head() {
        let channel_id = ChannelId::new();
        let (call, result) = call_and_result(channel_id);
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a"]));
        scrollback.push_call(call);
        scrollback.end_call(channel_id, &result);
        scrollback.push(lines(&["b", "c", "d"]));
        scrollback.visible(40, 2);
        scrollback.scroll_up(4);
        let seen = drawn(scrollback.visible(40, 2), 2);
        assert_eq!(seen[1], r#"▸ helper → f {"x":1}"#, "{seen:?}");
        scrollback.unfold_at(1);
        assert_eq!(drawn(scrollback.visible(40, 2), 2), ["a", r#"▾ helper → f {"x":1}"#]);
    }

    /// Following the end, a call unfolded keeps its head where what streams below takes no more
    /// rows than the call grows by; where it takes more, the end stays followed.
    #[test]
    fn following_the_end_a_call_unfolded_keeps_its_head_as_far_as_it_can() {
        let channel_id = ChannelId::new();
        let (call, result) = call_and_result(channel_id);
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["p", "q", "r"]));
        scrollback.push_call(call);
        scrollback.end_call(channel_id, &result);
        // Unfolded, the call grows by its arguments and its result: five rows.
        scrollback.set_live(lines(&["1", "2", "3", "4", "5"]));
        let seen = drawn(scrollback.visible(40, 7), 7);
        assert!(seen[0].starts_with('▸'), "{seen:?}");
        scrollback.unfold_at(0);
        let unfolded = drawn(scrollback.visible(40, 7), 7);
        assert!(unfolded[0].starts_with('▾'), "the head in place: {unfolded:?}");

        scrollback.unfold_at(0);
        scrollback.follow_end();
        scrollback.set_live(lines(&["1", "2", "3", "4", "5", "6"]));
        let seen = drawn(scrollback.visible(40, 8), 8);
        assert!(seen[0].starts_with('▸'), "{seen:?}");
        scrollback.unfold_at(0);
        assert_eq!(scrollback.below(), 0, "the end followed");
    }

    /// Scrolled up, a result joining a call whose head is the last row seen moves nothing.
    #[test]
    fn a_result_joining_a_call_at_the_bottom_moves_nothing() {
        let channel_id = ChannelId::new();
        let (call, result) = call_and_result(channel_id);
        let mut scrollback = Scrollback::default();
        scrollback.push(lines(&["a", "b"]));
        scrollback.push_call(call);
        scrollback.push(lines(&["c", "d"]));
        scrollback.visible(40, 2);
        scrollback.scroll_up(2);
        let seen = drawn(scrollback.visible(40, 2), 2);
        assert_eq!(seen, ["b", r#"▸ helper → f {"x":1}"#], "waiting for its result");
        scrollback.end_call(channel_id, &result);
        let seen = drawn(scrollback.visible(40, 2), 2);
        assert_eq!(seen[0], "b", "{seen:?}");
    }
}
