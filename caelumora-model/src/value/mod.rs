//! Values with rules (issue 248): a field whose text may only be what its
//! rules allow has a type of its own, made only through its constructor and
//! checked again when it is read - so that it cannot hold anything else,
//! wherever it comes from. One file per subject: `name` (`Name`,
//! `ScopedName`), `language` (`Language`), `time_zone` (`TimeZone`), `text`
//! (`Text<R>`, free text with a rule: `Summary`, `Briefing`,
//! `Introduction`, `Description`, `FileName`).
//!
//! What they share is here: one error, `ValueError`, naming the field, and
//! `impl_value!`, which gives every such type the same API next to its own
//! `new` and `as_str` - `FIELD`, `optional`, the conversions to and from a
//! `String`, and serde, as a string checked when read. None of them derefs
//! to `str`: a `Name` is compared by its folded form, and `&*name == text`
//! would compare it as written.
//!
//! A value is checked, never changed: it is kept exactly as written. What
//! identifies something - a name, a language - is compared by a key made
//! from it as well: "Dr.  Who" and "dr. who" are one name, `de-ch` and
//! `de-CH` one language, each kept as it was written.

/// Why a text does not fit its field. Every variant names the field; the
/// messages are meant for users.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    #[error("the {field} must not be empty")]
    Empty { field: &'static str },
    /// Of whitespace alone: not empty, but saying nothing.
    #[error("the {field} must not consist of whitespace alone")]
    Blank { field: &'static str },
    #[error("the {field} must not be longer than {max_chars} characters")]
    TooLong { field: &'static str, max_chars: usize },
    #[error("the {field} must be one line")]
    LineBreak { field: &'static str },
    #[error("the {field} must not contain control characters other than line breaks and tabs")]
    ControlCharacter { field: &'static str },
    /// Of a form the field does not take; `expected` says which it does.
    #[error("the {field} must be {expected}")]
    Malformed { field: &'static str, expected: &'static str },
}

/// The API every value type shares, for the type `$ty` - with its generic
/// parameters in front, in brackets: `impl_value!([] Name, "name")`. The
/// type has an inherent `new(impl AsRef<str>) -> Result<Self, ValueError>`
/// and `as_str(&self) -> &str`; `$field` is what its field is called.
macro_rules! impl_value {
    ([$($generics:tt)*] $ty:ty, $field:expr) => {
        impl<$($generics)*> $ty {
            /// What the field is called, in the errors.
            pub const FIELD: &'static str = $field;

            /// `text` as a value, `None` if it is missing or empty. Anything
            /// else is a value or an error, whitespace too: what is not
            /// empty is never taken for nothing.
            ///
            /// # Errors
            /// `ValueError` if it is not empty and no value (`new`).
            pub fn optional(text: Option<&str>) -> Result<Option<Self>, $crate::value::ValueError> {
                text.filter(|text| !text.is_empty()).map(Self::new).transpose()
            }
        }

        impl<$($generics)*> TryFrom<String> for $ty {
            type Error = $crate::value::ValueError;
            fn try_from(text: String) -> Result<Self, $crate::value::ValueError> {
                Self::new(text)
            }
        }

        impl<$($generics)*> From<$ty> for String {
            fn from(value: $ty) -> Self {
                value.as_str().to_string()
            }
        }

        impl<$($generics)*> AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl<$($generics)*> std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl<$($generics)*> std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(self.as_str(), f)
            }
        }

        /// Written down as a string.
        impl<$($generics)*> serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        /// Checked when read: a text that is no value is an error.
        impl<'de, $($generics)*> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::new(text).map_err(serde::de::Error::custom)
            }
        }
    };
}

pub mod language;
pub mod name;
pub mod text;
pub mod time_zone;
