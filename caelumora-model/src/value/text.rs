//! Free text people write for others to read (issue 248): `Text<R>`, checked
//! alike wherever it comes from - trimmed, not empty, bounded in length, and
//! without control characters that could break out of where it is placed.
//! How long it may be and whether it is one line is its rule `R`; the rules
//! are kept here together, since several owners share them: a session's,
//! a channel's and their templates' `Summary` and `Briefing`, a user's
//! `Introduction`.

use super::ValueError;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// What a field takes of a text: a marker type, named after the field.
pub trait TextRule {
    /// What the field is called, in the errors.
    const FIELD: &'static str;
    /// How many characters it may have.
    const MAX_CHARS: usize;
    /// Whether it is one line.
    const ONE_LINE: bool;
}

/// A text that fits the rule `R`. Made trimmed and never empty: a field
/// that may be left blank is an `Option`, and blank is `None` (`optional`).
/// Written down as a string, and checked when read.
pub struct Text<R> {
    text: String,
    rule: PhantomData<fn() -> R>,
}

impl<R: TextRule> Text<R> {
    /// How many characters it may have.
    pub const MAX_CHARS: usize = R::MAX_CHARS;

    /// `text` trimmed.
    ///
    /// # Errors
    /// `ValueError` if nothing is left of it, if it is longer than
    /// `MAX_CHARS`, contains a control character that is no whitespace, or
    /// - where it is one line - a line break.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref().trim();
        if text.is_empty() {
            return Err(ValueError::Empty { field: R::FIELD });
        }
        if text.chars().count() > R::MAX_CHARS {
            return Err(ValueError::TooLong { field: R::FIELD, max_chars: R::MAX_CHARS });
        }
        if R::ONE_LINE && text.chars().any(|c| c == '\n' || c == '\r') {
            return Err(ValueError::LineBreak { field: R::FIELD });
        }
        if text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
            return Err(ValueError::ControlCharacter { field: R::FIELD });
        }
        Ok(Self { text: text.to_string(), rule: PhantomData })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl_value!([R: TextRule] Text<R>, R::FIELD);

// By hand, since a derive would ask the same of the marker `R`.
impl<R> Clone for Text<R> {
    fn clone(&self) -> Self {
        Self { text: self.text.clone(), rule: PhantomData }
    }
}

impl<R> PartialEq for Text<R> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl<R> Eq for Text<R> {}

impl<R> Hash for Text<R> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

/// What a session, a channel or their template is for, in a line (issue
/// 245): seen at a glance.
pub enum SummaryRule {}

impl TextRule for SummaryRule {
    const FIELD: &'static str = "summary";
    const MAX_CHARS: usize = 300;
    const ONE_LINE: bool = true;
}

pub type Summary = Text<SummaryRule>;

/// What an agent should know before it works in a session or channel (issue
/// 245): it is in the context of every agent there on every turn.
pub enum BriefingRule {}

impl TextRule for BriefingRule {
    const FIELD: &'static str = "briefing";
    const MAX_CHARS: usize = 16_000;
    const ONE_LINE: bool = false;
}

pub type Briefing = Text<BriefingRule>;

/// How a user introduces themselves (issue 94): it goes into the system
/// prompt of every agent in every channel the user takes part in.
pub enum IntroductionRule {}

impl TextRule for IntroductionRule {
    const FIELD: &'static str = "introduction";
    const MAX_CHARS: usize = 1000;
    const ONE_LINE: bool = false;
}

pub type Introduction = Text<IntroductionRule>;

#[cfg(test)]
mod tests {
    use super::*;

    enum Line {}
    impl TextRule for Line {
        const FIELD: &'static str = "summary";
        const MAX_CHARS: usize = 10;
        const ONE_LINE: bool = true;
    }

    enum Lines {}
    impl TextRule for Lines {
        const FIELD: &'static str = "briefing";
        const MAX_CHARS: usize = 10;
        const ONE_LINE: bool = false;
    }

    #[test]
    fn a_text_is_trimmed_bounded_and_kept_free_of_control_characters() {
        assert_eq!(Text::<Lines>::new("  Be brief.\n").unwrap().as_str(), "Be brief.");
        assert_eq!(Text::<Line>::new("   "), Err(ValueError::Empty { field: "summary" }));
        assert_eq!(Text::<Line>::optional(Some("   ")), Ok(None), "blank is none");
        assert_eq!(Text::<Line>::optional(None), Ok(None));
        assert_eq!(Text::<Line>::new("eleven char"), Err(ValueError::TooLong { field: "summary", max_chars: 10 }));
        assert_eq!(Text::<Line>::new("one\ntwo"), Err(ValueError::LineBreak { field: "summary" }));
        assert!(Text::<Lines>::new("one\ntwo\tx").is_ok(), "lines and tabs where it is not one line");
        assert_eq!(Text::<Lines>::new("bell\u{7}"), Err(ValueError::ControlCharacter { field: "briefing" }));
        assert_eq!(ValueError::LineBreak { field: "summary" }.to_string(), "the summary must be one line", "naming the field");
    }

    #[test]
    fn a_text_is_written_as_a_string_and_checked_when_read() {
        let summary = Summary::new("Release 2.0").unwrap();
        assert_eq!(serde_json::to_value(&summary).unwrap(), serde_json::json!("Release 2.0"));
        assert_eq!(serde_json::from_value::<Summary>(serde_json::json!("Release 2.0")).unwrap(), summary);
        assert!(serde_json::from_value::<Summary>(serde_json::json!("one\ntwo")).is_err(), "a summary is one line");
        assert!(serde_json::from_value::<Summary>(serde_json::json!("")).is_err(), "only none is stored for nothing");
    }
}
