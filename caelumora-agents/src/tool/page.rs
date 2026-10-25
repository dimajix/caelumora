//! A text given a page at a time (issue 282): what a tool that reads a text - a workspace's file,
//! an attachment - gives its model of it at once, and where it reads on.

use super::{result_text, ToolError, ToolResult};
use caelumora_model::prompt_catalogue::{context, PromptTexts};
use serde_json::{json, Value};
use tracing::error;

/// The lines of a text a tool gives its model at once: from `start_line` through `end_line`, of
/// `total_lines`, and where to read on if what was asked for goes beyond them.
#[derive(Debug, PartialEq, Eq)]
pub struct TextPage {
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    /// The line to read on from, if what was asked for goes beyond the page.
    pub next_line: Option<usize>,
}

impl TextPage {
    /// The page as a tool's result gives it - its content, the lines it covers, and where to read
    /// on, worded by `prompts` (`results.read_on`) -, for the tool to add what the text is.
    pub fn value(self, prompts: &PromptTexts) -> Value {
        let mut value = json!({ "content": self.content, "start_line": self.start_line, "end_line": self.end_line, "total_lines": self.total_lines });
        if let Some(line) = self.next_line {
            match result_text(prompts, "read_on", context! { line }) {
                Ok(next) => value["next"] = json!(next),
                Err(e) => error!("A page of a text is given without saying where to read on: {e}"),
            }
        }
        value
    }
}

/// The page of `text` from line `start_line` - the first if `None` - of at most `line_count` lines:
/// as many whole lines as `max_bytes` holds, one at least.
///
/// # Errors
/// `InvalidParameter` if `start_line` lies beyond the text, which the message names as `what`.
pub fn page_of(text: &str, what: &str, start_line: Option<usize>, line_count: Option<usize>, max_bytes: usize) -> ToolResult<TextPage> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = start_line.unwrap_or(1).max(1);
    if start > lines.len().max(1) {
        return Err(ToolError::InvalidParameter(format!("{what} has {} lines; start_line {start} is beyond them", lines.len())));
    }
    let wanted = lines.iter().skip(start - 1).take(line_count.unwrap_or(usize::MAX));
    let (mut content, mut end) = (String::new(), start - 1);
    for line in wanted {
        if content.len() + line.len() > max_bytes && end >= start {
            break;
        }
        content.push_str(line);
        end += 1;
    }
    let asked_end = line_count.map_or(lines.len(), |count| (start - 1 + count).min(lines.len()));
    let next_line = (end < asked_end).then_some(end + 1);
    Ok(TextPage { content, start_line: start, end_line: end, total_lines: lines.len(), next_line })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_holds_whole_lines_within_its_bytes_and_says_where_to_read_on() {
        let text = "line one\nline two\nline three\n";
        let page = |start, count, bytes| page_of(text, "'long.txt'", start, count, bytes);
        assert_eq!(page(None, None, 10).unwrap(), TextPage { content: "line one\n".to_string(), start_line: 1, end_line: 1, total_lines: 3, next_line: Some(2) }, "one line at least, and where to go on");
        assert_eq!(page(Some(2), Some(1), 100).unwrap(), TextPage { content: "line two\n".to_string(), start_line: 2, end_line: 2, total_lines: 3, next_line: None }, "no more than asked for");
        assert_eq!(page(Some(3), None, 100).unwrap().next_line, None, "to the end");
        let beyond = page(Some(9), None, 100).unwrap_err().to_string();
        assert!(beyond.contains("'long.txt' has 3 lines"), "{beyond}");
        assert_eq!(page_of("", "'empty'", None, None, 10).unwrap().content, "", "an empty text is one page");
    }
}
