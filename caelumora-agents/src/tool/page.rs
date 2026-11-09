//! A text given a page at a time (issues 282, 546): what a tool that reads a text - a workspace's
//! file, an attachment, a page of the web - gives its model of it at once, and where it reads on.

use super::{result_text, ToolError, ToolResult};
use caelumora_domain::prompt_catalogue::{context, PromptTexts};
use caelumora_domain::tool::ToolParameters;
use serde_json::{json, Value};
use tracing::error;

/// A place in a text: a line, counted from one, and how many bytes of it lie before the place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub line: usize,
    pub offset: usize,
}

impl TextPosition {
    /// The place a call of a tool names: its `start_line`, the first if `None`, and its
    /// `start_offset` there, none if `None`.
    pub fn asked(start_line: Option<usize>, start_offset: Option<usize>) -> Self {
        Self { line: start_line.unwrap_or(1), offset: start_offset.unwrap_or(0) }
    }
}

/// The lines of a text a tool gives its model at once: from `start_line` through `end_line`, of
/// `total_lines`, and where to read on if what was asked for goes beyond them.
#[derive(Debug, PartialEq, Eq)]
pub struct TextPage {
    pub content: String,
    pub start_line: usize,
    /// How many bytes of `start_line` lie before the page.
    pub start_offset: usize,
    pub end_line: usize,
    pub total_lines: usize,
    /// Where to read on, if what was asked for goes beyond the page.
    pub next: Option<TextPosition>,
}

impl TextPage {
    /// The page as a tool's result gives it - its content, the lines it covers, and where to read
    /// on, worded by `prompts` (`results.read_on`, inside a line `results.read_on_within`) -, for
    /// the tool to add what the text is.
    pub fn value(self, prompts: &PromptTexts) -> Value {
        let mut value = json!({ "content": self.content, "start_line": self.start_line, "end_line": self.end_line, "total_lines": self.total_lines });
        if self.start_offset > 0 {
            value["start_offset"] = json!(self.start_offset);
        }
        if let Some(TextPosition { line, offset }) = self.next {
            let next = match offset {
                0 => result_text(prompts, "read_on", context! { line }),
                _ => result_text(prompts, "read_on_within", context! { line, offset }),
            };
            match next {
                Ok(next) => value["next"] = json!(next),
                Err(e) => error!("A page of a text is given without saying where to read on: {e}"),
            }
        }
        value
    }
}

/// The parameters of a tool that reads a page of a text, which its own texts describe.
pub fn page_parameters() -> [(&'static str, ToolParameters); 3] {
    [
        ("start_line", ToolParameters::number()),
        ("start_offset", ToolParameters::number()),
        ("line_count", ToolParameters::number()),
    ]
}

/// The page of `text` from `start`, of at most `line_count` lines, the one begun counted among
/// them: as many whole lines as `max_bytes` holds. A line that does not fit on a page by itself is
/// cut on a character boundary, and the page's `next` lies inside it; a character is given whatever
/// `max_bytes` says, so that reading on gets somewhere. An offset inside a character means that
/// character.
///
/// # Errors
/// `InvalidParameter` if `start` lies beyond the text, which the message names as `what`.
pub fn page_of(
    text: &str,
    what: &str,
    start: TextPosition,
    line_count: Option<usize>,
    max_bytes: usize,
) -> ToolResult<TextPage> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first = start.line.max(1);
    if first > lines.len().max(1) {
        return Err(ToolError::InvalidParameter(format!(
            "{what} has {} lines; start_line {first} is beyond them",
            lines.len()
        )));
    }
    let first_line = lines.get(first - 1).copied().unwrap_or_default();
    if start.offset > first_line.len() {
        return Err(ToolError::InvalidParameter(format!(
            "line {first} of {what} has {} bytes; start_offset {} is beyond them",
            first_line.len(),
            start.offset
        )));
    }
    let start_offset = first_line.floor_char_boundary(start.offset);
    Ok(page_within(
        &lines,
        TextPosition { line: first, offset: start_offset },
        line_count,
        max_bytes,
    ))
}

/// The first page of `text`, of as many whole lines as `max_bytes` holds, as `page_of` gives it.
pub(super) fn beginning_of(text: &str, max_bytes: usize) -> TextPage {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    page_within(&lines, TextPosition::asked(None, None), None, max_bytes)
}

/// The page of `lines` from `start`, which lies within them, on a character boundary.
fn page_within(
    lines: &[&str],
    start: TextPosition,
    line_count: Option<usize>,
    max_bytes: usize,
) -> TextPage {
    let TextPosition { line: first, offset: start_offset } = start;
    let wanted = lines.iter().skip(first - 1).take(line_count.unwrap_or(usize::MAX));
    let mut content = String::new();
    let mut end = first - 1;
    let mut next = None;
    for (line, whole) in (first..).zip(wanted) {
        let before = if line == first { start_offset } else { 0 };
        let rest = &whole[before..];
        if content.len() + rest.len() > max_bytes {
            next = Some(if content.is_empty() {
                let cut = cut_of(rest, max_bytes);
                content.push_str(&rest[..cut]);
                end = line;
                TextPosition { line, offset: before + cut }
            } else {
                TextPosition { line, offset: 0 }
            });
            break;
        }
        content.push_str(rest);
        end = line;
    }
    let total_lines = lines.len();
    TextPage { content, start_line: first, start_offset, end_line: end, total_lines, next }
}

/// How many bytes of `line` a page of `max_bytes` takes: as many whole characters as it holds, one
/// at least.
fn cut_of(line: &str, max_bytes: usize) -> usize {
    match line.floor_char_boundary(max_bytes) {
        0 => line.chars().next().map_or(0, char::len_utf8),
        cut => cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pages of `text` from its beginning, each read on from where the one before said.
    fn pages(text: &str, max_bytes: usize) -> Vec<TextPage> {
        let mut pages = Vec::new();
        let mut start = TextPosition::asked(None, None);
        loop {
            let page = page_of(text, "'text'", start, None, max_bytes).unwrap();
            let next = page.next;
            pages.push(page);
            match next {
                Some(next) => start = next,
                None => return pages,
            }
        }
    }

    #[test]
    fn a_page_holds_whole_lines_within_its_bytes_and_says_where_to_read_on() {
        let text = "line one\nline two\nline three\n";
        let page = |start, count, bytes| {
            page_of(text, "'long.txt'", TextPosition::asked(start, None), count, bytes)
        };
        assert_eq!(
            page(None, None, 10).unwrap(),
            TextPage {
                content: "line one\n".to_string(),
                start_line: 1,
                start_offset: 0,
                end_line: 1,
                total_lines: 3,
                next: Some(TextPosition { line: 2, offset: 0 })
            },
            "a line that fits on a page of its own begins the next"
        );
        assert_eq!(
            page(Some(2), Some(1), 100).unwrap(),
            TextPage {
                content: "line two\n".to_string(),
                start_line: 2,
                start_offset: 0,
                end_line: 2,
                total_lines: 3,
                next: None
            },
            "no more than asked for"
        );
        assert_eq!(page(Some(3), None, 100).unwrap().next, None, "to the end");
        let beyond = page(Some(9), None, 100).unwrap_err().to_string();
        assert!(beyond.contains("'long.txt' has 3 lines"), "{beyond}");
        assert_eq!(
            page_of("", "'empty'", TextPosition::asked(None, None), None, 10).unwrap().content,
            "",
            "an empty text is one page"
        );
    }

    #[test]
    fn a_line_longer_than_a_page_is_cut_and_read_on_inside_it() {
        let text = "abcdefghij\nxy\n";
        let pages = pages(text, 4);
        let contents: Vec<&str> = pages.iter().map(|page| page.content.as_str()).collect();
        assert_eq!(contents, ["abcd", "efgh", "ij\n", "xy\n"], "nothing lost, nothing twice");
        assert_eq!(pages[0].next, Some(TextPosition { line: 1, offset: 4 }));
        assert_eq!((pages[1].start_line, pages[1].start_offset, pages[1].end_line), (1, 4, 1));
        assert_eq!(pages[2].next, Some(TextPosition { line: 2, offset: 0 }), "the next line whole");
    }

    #[test]
    fn a_page_of_a_cut_line_holds_no_more_than_its_bytes_and_is_of_the_lines_asked_for() {
        let text = format!("{}\nsecond\n", "x".repeat(1000));
        let start = TextPosition::asked(None, None);
        let page = page_of(&text, "'text'", start, Some(1), 64).unwrap();
        assert_eq!(page.content.len(), 64);
        assert_eq!(page.next, Some(TextPosition { line: 1, offset: 64 }), "the line goes on");
        let start = TextPosition { line: 1, offset: 992 };
        let last = page_of(&text, "'text'", start, Some(1), 64).unwrap();
        assert_eq!(last.content.len(), 9, "the end of the line");
        assert_eq!(last.next, None, "and no further than the one line asked for");
    }

    #[test]
    fn a_page_begun_inside_a_line_goes_on_with_the_lines_after_it() {
        let start = TextPosition { line: 1, offset: 4 };
        let page = page_of("abcdef\nxy\nz\n", "'text'", start, None, 10).unwrap();
        assert_eq!(page.content, "ef\nxy\nz\n");
        assert_eq!((page.start_line, page.start_offset, page.end_line), (1, 4, 3));
        assert_eq!(page.next, None);
        let at_its_end = TextPosition { line: 1, offset: 3 };
        let page = page_of("ab\ncd\n", "'text'", at_its_end, None, 10).unwrap();
        assert_eq!(
            (page.content.as_str(), page.end_line),
            ("cd\n", 2),
            "at a line's end, the next"
        );
    }

    #[test]
    fn a_line_is_cut_between_characters() {
        let text = "äöü\n";
        let contents: Vec<String> = pages(text, 3).into_iter().map(|page| page.content).collect();
        assert_eq!(contents, ["ä", "ö", "ü\n"]);
        let inside = page_of(text, "'text'", TextPosition { line: 1, offset: 3 }, None, 2).unwrap();
        assert_eq!((inside.content.as_str(), inside.start_offset), ("ö", 2), "the whole character");
    }

    #[test]
    fn reading_on_gets_somewhere_on_a_page_smaller_than_a_character() {
        let contents: Vec<String> = pages("äb", 1).into_iter().map(|page| page.content).collect();
        assert_eq!(contents, ["ä", "b"]);
    }

    #[test]
    fn an_offset_beyond_its_line_is_the_models_to_correct() {
        let start = TextPosition { line: 1, offset: 9 };
        let beyond = page_of("ab\ncd\n", "'text'", start, None, 10).unwrap_err().to_string();
        assert!(beyond.contains("line 1 of 'text' has 3 bytes"), "{beyond}");
        let beyond = page_of("", "'empty'", start, None, 10).unwrap_err().to_string();
        assert!(beyond.contains("line 1 of 'empty' has 0 bytes"), "{beyond}");
    }
}
