//! What the configuration names things by (issue 512): `Identifier`, lower case letters, digits,
//! `_` and `-` - a variant of the texts, a workflow of ComfyUI.

use super::ValueError;
use std::borrow::Borrow;

/// What an identifier looks like, in the error for one that does not.
const EXPECTED: &str = "lower case letters, digits, '_' and '-'";

/// A name the configuration gives something and refers to it by: lower case letters, digits, `_`
/// and `-`, never empty - so that it is written one way only, and compared as written. Written
/// down as a string, and checked when read.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identifier(String);

impl Identifier {
    /// `text` as an identifier.
    ///
    /// # Errors
    /// `ValueError` if it is empty or holds anything but the characters above.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref();
        if text.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        if !text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(ValueError::Malformed { field: Self::FIELD, expected: EXPECTED });
        }
        Ok(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_value!([] Identifier, "identifier");

/// Found by its text: compared as written.
impl Borrow<str> for Identifier {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_is_lower_case_letters_digits_underscores_and_dashes() {
        for name in ["z_image_turbo", "gemma4", "flux-dev", "a"] {
            assert_eq!(Identifier::new(name).unwrap().as_str(), name);
        }
        assert!(matches!(Identifier::new(""), Err(ValueError::Empty { .. })));
        for name in ["Z_image", "z image", "z.image", "zäh"] {
            assert!(matches!(Identifier::new(name), Err(ValueError::Malformed { .. })), "{name}");
        }
        let read: Identifier = serde_json::from_value(serde_json::json!("flux")).unwrap();
        assert_eq!(read.as_str(), "flux");
        assert!(serde_json::from_value::<Identifier>(serde_json::json!("Flux")).is_err());
    }
}
