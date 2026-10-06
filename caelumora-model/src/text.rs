//! Free text people write for others to read - checked alike wherever it
//! comes from: trimmed, nothing if nothing is left, bounded in length, and
//! without control characters that could break out of where it is placed.
//! How long a text may be and whether it is one line is the rule of the
//! field it is written into, kept by the field's owner (`TextRule`); this
//! module knows no field.

/// What a field takes of a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRule {
    /// What the field is called, for the user told what is wrong.
    pub field: &'static str,
    /// How many characters it may have.
    pub max_chars: usize,
    /// Whether it is one line.
    pub one_line: bool,
}

impl TextRule {
    /// The field `field`: one line of at most `max_chars` characters.
    pub const fn line(field: &'static str, max_chars: usize) -> Self {
        Self { field, max_chars, one_line: true }
    }

    /// The field `field`: any number of lines, at most `max_chars`
    /// characters in all.
    pub const fn lines(field: &'static str, max_chars: usize) -> Self {
        Self { field, max_chars, one_line: false }
    }

    /// `text` trimmed, `None` if nothing is left of it.
    ///
    /// # Errors
    /// `TextError` if it is longer than `max_chars`, contains a control
    /// character that is no whitespace, or - where it is one line - a line
    /// break.
    pub fn check(&self, text: Option<&str>) -> Result<Option<String>, TextError> {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else { return Ok(None) };
        if text.chars().count() > self.max_chars {
            return Err(TextError::TooLong { field: self.field, max_chars: self.max_chars });
        }
        if self.one_line && text.chars().any(|c| c == '\n' || c == '\r') {
            return Err(TextError::LineBreak { field: self.field });
        }
        if text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
            return Err(TextError::ControlCharacter { field: self.field });
        }
        Ok(Some(text.to_string()))
    }
}

/// Why a text does not fit its field. The messages are meant for users.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("the {field} must not be longer than {max_chars} characters")]
    TooLong { field: &'static str, max_chars: usize },
    #[error("the {field} must be one line")]
    LineBreak { field: &'static str },
    #[error("the {field} must not contain control characters other than line breaks and tabs")]
    ControlCharacter { field: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_is_trimmed_bounded_and_kept_free_of_control_characters() {
        let (line, lines) = (TextRule::line("summary", 10), TextRule::lines("briefing", 10));
        assert_eq!(lines.check(Some("  Be brief.\n")), Ok(Some("Be brief.".to_string())));
        assert_eq!(line.check(Some("   ")), Ok(None), "nothing left: none");
        assert_eq!(line.check(None), Ok(None));
        assert_eq!(line.check(Some("eleven char")), Err(TextError::TooLong { field: "summary", max_chars: 10 }));
        assert_eq!(line.check(Some("one\ntwo")), Err(TextError::LineBreak { field: "summary" }));
        assert!(lines.check(Some("one\ntwo\tx")).is_ok(), "lines and tabs where it is not one line");
        assert_eq!(lines.check(Some("bell\u{7}")), Err(TextError::ControlCharacter { field: "briefing" }));
        assert_eq!(TextError::LineBreak { field: "summary" }.to_string(), "the summary must be one line", "naming the field");
    }
}
