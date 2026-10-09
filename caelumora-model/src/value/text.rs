//! Free text people write for others to read (issue 248): `Text<R>`, checked
//! alike wherever it comes from - not empty, not of whitespace alone,
//! bounded in length, and without control characters that could break out
//! of where it is placed - and kept exactly as written: it is checked, not
//! changed. Whoever wants it trimmed trims it where it is entered.
//! How long it may be and whether it is one line is its rule `R`; the rules
//! are kept here together, since several owners share them: a session's,
//! a channel's and their templates' `Summary` and `Briefing`, a user's and
//! a personality's `Introduction`, a personality's and a session template's
//! `Description`.

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

/// A text that fits the rule `R`, as written. Never empty: a field that may
/// be left empty is an `Option`, and empty is `None` (`optional`).
/// Written down as a string, and checked when read.
pub struct Text<R> {
    text: String,
    rule: PhantomData<fn() -> R>,
}

impl<R: TextRule> Text<R> {
    /// How many characters it may have.
    pub const MAX_CHARS: usize = R::MAX_CHARS;

    /// `text` as it is written.
    ///
    /// # Errors
    /// `ValueError` if it is empty or of whitespace alone, if it is longer
    /// than `MAX_CHARS`, contains a control character that is no whitespace,
    /// or - where it is one line - a line break.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref();
        if text.is_empty() {
            return Err(ValueError::Empty { field: R::FIELD });
        }
        if text.trim().is_empty() {
            return Err(ValueError::Blank { field: R::FIELD });
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

    /// `text`, unchecked: for a text the code itself fixes, which a test
    /// checks instead (see `Name::fixed`).
    pub(super) fn fixed(text: &'static str) -> Self {
        Self { text: text.to_string(), rule: PhantomData }
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

/// What a personality or a session template is, for whoever chooses one: a
/// catalogue line; what a workspace holds, as its owner describes it.
pub enum DescriptionRule {}

impl TextRule for DescriptionRule {
    const FIELD: &'static str = "description";
    const MAX_CHARS: usize = 300;
    const ONE_LINE: bool = true;
}

pub type Description = Text<DescriptionRule>;

/// What a skill tells the model to do (issue 22): instructions in Markdown,
/// loaded into a turn when the skill is used - twice a briefing, enough for
/// a procedure, not for a manual.
pub enum SkillBodyRule {}

impl TextRule for SkillBodyRule {
    const FIELD: &'static str = "body";
    const MAX_CHARS: usize = 32_000;
    const ONE_LINE: bool = false;
}

pub type SkillBody = Text<SkillBodyRule>;

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
    fn a_text_is_bounded_and_kept_free_of_control_characters_and_as_written() {
        assert_eq!(Text::<Lines>::new(" Brief.\n").unwrap().as_str(), " Brief.\n", "checked, not changed");
        assert_eq!(Text::<Line>::new(""), Err(ValueError::Empty { field: "summary" }));
        for blank in [" ", "\t", "\n", "\r\n", " \n "] {
            assert_eq!(Text::<Lines>::new(blank), Err(ValueError::Blank { field: "briefing" }), "{blank:?}");
        }
        assert_eq!(Text::<Line>::optional(Some("")), Ok(None), "empty is none");
        assert_eq!(Text::<Line>::optional(Some(" ")), Err(ValueError::Blank { field: "summary" }), "whitespace is refused, not taken for nothing");
        assert_eq!(Text::<Line>::optional(None), Ok(None));
        assert_eq!(Text::<Line>::new("eleven char"), Err(ValueError::TooLong { field: "summary", max_chars: 10 }));
        assert_eq!(Text::<Line>::new("one\ntwo"), Err(ValueError::LineBreak { field: "summary" }));
        assert_eq!(Text::<Line>::new("one\n"), Err(ValueError::LineBreak { field: "summary" }), "a line break at its end, too");
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
