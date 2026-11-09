//! The preview that stands in for a text too long for a tool's result (issues 547, 552): from how
//! many bytes on the whole is kept beside it (`KeepWholeAbove`, a parameter of the components that
//! do so), what the tool then gives (`ToolOutput::kept`), and a text cut to that size which
//! is not kept (`cut_to_preview`).

use super::page::beginning_of;
use super::{result_text, TextPage, ToolOutput};
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
    /// What is too long for a result kept as `files`, the value being the beginning of `preview`,
    /// if there is a text to give one of, and a `next` that says, in the words of
    /// `results.kept_whole`, how the whole is read. Without the words, logged, the result goes
    /// without saying so.
    pub fn kept(preview: Option<&str>, files: Vec<ToolContent>, prompts: &PromptTexts) -> Self {
        let mut value = match preview {
            Some(text) => {
                TextPage { next: None, ..beginning_of(text, MAX_PREVIEW_BYTES) }.value(prompts)
            }
            None => json!({}),
        };
        match result_text(prompts, "kept_whole", ()) {
            Ok(next) => value["next"] = json!(next),
            Err(e) => error!("A text kept whole is given without saying how it is read: {e}"),
        }
        Self { files, ..value.into() }
    }
}

/// `text`, or its beginning and `...` if it is longer than a preview holds: for what is not worth
/// keeping and may still be long, such as what a failure says.
pub fn cut_to_preview(text: String) -> String {
    let beginning = beginning_of(&text, MAX_PREVIEW_BYTES);
    match beginning.next {
        Some(_) => format!("{}...", beginning.content),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::prompts;

    #[test]
    fn a_text_is_cut_only_if_a_preview_does_not_hold_it() {
        let short = "x".repeat(MAX_PREVIEW_BYTES);
        assert_eq!(cut_to_preview(short.clone()), short);
        assert_eq!(cut_to_preview(format!("{short}y")), format!("{short}..."));
    }

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
        let files = vec![ToolContent { name: "long.txt".to_string(), bytes: text.clone().into() }];
        let output = ToolOutput::kept(Some(&text), files, &prompts());
        let [kept] = output.files.as_slice() else { panic!("one file: {:?}", output.files) };
        assert_eq!(kept.name, "long.txt");
        assert_eq!(kept.bytes.as_ref(), text.as_bytes(), "byte for byte");
        assert_eq!(output.value["content"].as_str().map(str::len), Some(MAX_PREVIEW_BYTES));
        let next = output.value["next"].as_str().unwrap();
        assert!(next.contains("attachment") && !next.contains("start_"), "{next}");
        assert_eq!(output.classification, None, "the tool's to classify");
    }

    #[test]
    fn what_is_kept_without_a_text_says_only_how_it_is_read() {
        let files = vec![ToolContent { name: "data.json".to_string(), bytes: "{}".into() }];
        let output = ToolOutput::kept(None, files, &prompts());
        assert_eq!(output.files.len(), 1);
        let value = output.value.as_object().unwrap();
        assert_eq!(value.keys().collect::<Vec<_>>(), ["next"], "{value:?}");
    }
}
