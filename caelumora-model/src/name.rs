//! Names that things are looked up by (issue 177): `Name`, and `ScopedName`
//! for a definition that is found by name within its scope. A name that is
//! only shown - a session's - stays a `String`; one that is decided
//! elsewhere - a user's, which a user provider such as a directory owns -
//! too: it is opaque to Caelumora.

use crate::mention::handle_of;
use crate::scope::Scope;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

/// The longest name, in characters: a name stands in prompts and lists.
pub const MAX_NAME_CHARS: usize = 100;

/// Why a text is no name. The messages are meant for users.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("a name must not be empty")]
    Empty,
    #[error("a name must not contain control characters")]
    ControlCharacter,
    #[error("a name must not be longer than {MAX_NAME_CHARS} characters")]
    TooLong,
}

/// A name that something is looked up by: an agent's, a personality's, a
/// session template's, a channel's. Kept as written, with its whitespace
/// tidied - no leading or trailing whitespace, every run of it one space -
/// and compared regardless of case: "Jonathan" and "jonathan" are one name,
/// in a lookup, in a map and as a key. Written down as a string, and checked
/// when read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);

impl Name {
    /// `text` as a name, its whitespace tidied.
    ///
    /// # Errors
    /// `NameError` if nothing is left of it, if it contains a control
    /// character, or if it is longer than `MAX_NAME_CHARS`.
    pub fn new(text: impl AsRef<str>) -> Result<Self, NameError> {
        let name = text.as_ref().split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        if name.chars().any(char::is_control) {
            return Err(NameError::ControlCharacter);
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(NameError::TooLong);
        }
        Ok(Self(name))
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The handle the name is `@`-mentioned by (see `crate::mention`);
    /// empty if it has no character a handle may have.
    pub fn handle(&self) -> String {
        handle_of(&self.0)
    }

    /// What the name is compared and hashed by.
    fn key(&self) -> String {
        self.0.to_lowercase()
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Name {}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

/// A text a lookup was asked with, e.g. by a tool: the name it would be,
/// compared regardless of case. A text that is no name is none of them.
impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        Name::new(other).is_ok_and(|other| *self == other)
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl TryFrom<String> for Name {
    type Error = NameError;
    fn try_from(text: String) -> Result<Self, NameError> {
        Name::new(text)
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// The name of a definition and the scope it lies in (issue 177): what it
/// is found by. Written into the definition's document as its fields `name`
/// and `scope` - flattened - the scope global unless named.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopedName {
    pub name: Name,
    #[serde(default)]
    pub scope: Scope,
}

impl ScopedName {
    /// `name` as a name in `scope`.
    ///
    /// # Errors
    /// `NameError` if `name` is no name (`Name::new`).
    pub fn new(name: impl AsRef<str>, scope: Scope) -> Result<Self, NameError> {
        Ok(Self { name: Name::new(name)?, scope })
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }
}

/// The name, without its scope: how a definition is named to people.
impl fmt::Display for ScopedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.name, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::new(text).expect("a name")
    }

    #[test]
    fn a_name_is_kept_as_written_with_its_whitespace_tidied() {
        assert_eq!(name("  Dr.   Who ").as_str(), "Dr. Who");
        assert_eq!(Name::new(" \t "), Err(NameError::Empty));
        assert_eq!(Name::new("a\u{7}b"), Err(NameError::ControlCharacter));
        assert_eq!(Name::new("x".repeat(MAX_NAME_CHARS + 1)), Err(NameError::TooLong));
        assert!(Name::new("x".repeat(MAX_NAME_CHARS)).is_ok());
    }

    #[test]
    fn names_are_one_regardless_of_case_in_a_lookup_and_a_map() {
        assert_eq!(name("Jonathan"), name("jonathan"));
        assert!(name("Zoë") == "ZOË", "beyond ASCII, too");
        assert!(name("Jonathan") != "", "a text that is no name is none");
        let set: std::collections::HashSet<Name> = [name("Sassy"), name("sassy")].into_iter().collect();
        assert_eq!(set.len(), 1);
        assert_eq!(name("Dr. Who").handle(), "dr-who");
    }

    #[test]
    fn a_name_is_written_as_a_string_and_checked_when_read() {
        assert_eq!(serde_json::to_value(name("Sassy")).unwrap(), serde_json::json!("Sassy"));
        assert!(serde_json::from_value::<Name>(serde_json::json!("")).is_err());

        #[derive(Debug, Serialize, Deserialize)]
        struct Definition {
            #[serde(flatten)]
            name: ScopedName,
        }
        let read: Definition = serde_json::from_value(serde_json::json!({ "name": "Jonathan" })).unwrap();
        assert_eq!(read.name, ScopedName::new("jonathan", Scope::GlobalScope).unwrap(), "global unless named");
        let written = serde_json::to_value(&read).unwrap();
        assert_eq!(written, serde_json::json!({ "name": "Jonathan", "scope": "GlobalScope" }));
    }
}
