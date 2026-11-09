//! A text too long for a tool's result, kept whole beside a preview of it (issues 547, 552): from
//! how many bytes on (`KeepWholeAbove`, a parameter of the components that do so), and what the
//! tool then gives (`ToolOutput::kept_whole`).

use super::page::beginning_of;
use super::{result_text, TextPage, ToolOutput};
use bytes::Bytes;
use caelumora_domain::prompt_catalogue::PromptTexts;
use caelumora_domain::tool::ToolContent;
use serde::Deserialize;
use serde_json::json;
use tracing::error;

/// How many bytes of a text kept whole a result gives in its stead.
const MAX_PREVIEW_BYTES: usize = 4 * 1024;

/// From how many bytes on a text is kept whole; refuses less than a preview when it is read, which
/// could not say less than the text. A component's `keep_whole_above_bytes`: whoever sets it knows
/// that the agent has the tools to reach what is kept, and that there is a storage to keep it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "usize")]
pub struct KeepWholeAbove(usize);

impl KeepWholeAbove {
    /// Whether `text` is above the bound.
    pub fn keeps(self, text: &str) -> bool {
        text.len() > self.0
    }
}

impl TryFrom<usize> for KeepWholeAbove {
    type Error = String;

    fn try_from(bytes: usize) -> Result<Self, Self::Error> {
        if bytes < MAX_PREVIEW_BYTES {
            return Err(format!("it must be at least {MAX_PREVIEW_BYTES}, the size of a preview"));
        }
        Ok(Self(bytes))
    }
}

impl ToolOutput {
    /// A preview of `text` as the value, and `text` whole as content named `name`: its beginning,
    /// and a `next` that says, in the words of `results.kept_whole`, how the whole is read. Without
    /// the words, logged, the result goes without saying so.
    pub fn kept_whole(text: String, name: String, prompts: &PromptTexts) -> Self {
        let preview = TextPage { next: None, ..beginning_of(&text, MAX_PREVIEW_BYTES) };
        let mut value = preview.value(prompts);
        match result_text(prompts, "kept_whole", ()) {
            Ok(next) => value["next"] = json!(next),
            Err(e) => error!("A text kept whole is given without saying how it is read: {e}"),
        }
        Self { files: vec![ToolContent { name, bytes: Bytes::from(text) }], ..value.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::prompts;

    #[test]
    fn a_bound_is_at_least_a_preview_and_keeps_what_lies_above_it() {
        let read = |bytes: usize| serde_json::from_value::<KeepWholeAbove>(json!(bytes));
        let refused = read(MAX_PREVIEW_BYTES - 1).unwrap_err().to_string();
        assert!(refused.contains("at least 4096"), "{refused}");
        let above = read(MAX_PREVIEW_BYTES).unwrap();
        assert!(!above.keeps(&"x".repeat(MAX_PREVIEW_BYTES)), "at the bound");
        assert!(above.keeps(&"x".repeat(MAX_PREVIEW_BYTES + 1)));
    }

    #[test]
    fn a_text_kept_is_given_whole_beside_a_preview_that_says_how_it_is_read() {
        let text = format!("\u{1b}[1m{}\n", "x".repeat(MAX_PREVIEW_BYTES * 2));
        let output = ToolOutput::kept_whole(text.clone(), "long.txt".to_string(), &prompts());
        let [kept] = output.files.as_slice() else { panic!("one file: {:?}", output.files) };
        assert_eq!(kept.name, "long.txt");
        assert_eq!(kept.bytes.as_ref(), text.as_bytes(), "byte for byte");
        assert_eq!(output.value["content"].as_str().map(str::len), Some(MAX_PREVIEW_BYTES));
        let next = output.value["next"].as_str().unwrap();
        assert!(next.contains("attachment") && !next.contains("start_"), "{next}");
        assert_eq!(output.classification, None, "the tool's to classify");
    }
}
