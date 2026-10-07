//! Names that things are looked up by (issue 177): `Name`, and `ScopedName`
//! for a definition that is found by name within its scope. A name that is
//! only shown - a session's - stays a `String`; one that is decided
//! elsewhere - a user's, which a user provider such as a directory owns -
//! too: it is opaque to Caelumora.

use super::ValueError;
use crate::mention::handle_of;
use crate::scope::Scope;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

/// The longest name, in characters: a name stands in prompts and lists.
pub const MAX_NAME_CHARS: usize = 100;

/// A name that something is looked up by: an agent's, a personality's, a
/// session template's, a channel's. Kept as written, with its whitespace
/// tidied - no leading or trailing whitespace, every run of it one space -
/// and compared by its ASCII form regardless of case (`fold`): "Jonathan",
/// "jonathan" and "JONATHAN" are one name, and so are "Zoë" and "Zoe", in a
/// lookup, in a map and as a key. Written down as a string, and checked when
/// read.
#[derive(Clone)]
pub struct Name {
    text: String,
    /// What it is compared and hashed by, made once with the name: a name
    /// never changes.
    key: String,
}

impl Name {
    /// `text` as a name, its whitespace tidied.
    ///
    /// # Errors
    /// `ValueError` if nothing is left of it, if it contains a control
    /// character, or if it is longer than `MAX_NAME_CHARS`.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let name = text.as_ref().split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        if name.chars().any(char::is_control) {
            return Err(ValueError::ControlCharacter { field: Self::FIELD });
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(ValueError::TooLong { field: Self::FIELD, max_chars: MAX_NAME_CHARS });
        }
        let key = fold(&name);
        Ok(Self { text: name, key })
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The handle the name is `@`-mentioned by (see `crate::mention`);
    /// empty if it has no character a handle may have.
    pub fn handle(&self) -> String {
        handle_of(&self.text)
    }
}

/// What a name is compared by: transliterated to ASCII (`any_ascii` - the
/// web UI uses the npm package of the same name), in lower case, every run
/// of whitespace one space. A name of which nothing is left so is compared
/// by its lower case.
fn fold(name: &str) -> String {
    let ascii = any_ascii::any_ascii(name).to_lowercase();
    let key = ascii.split_whitespace().collect::<Vec<_>>().join(" ");
    if key.is_empty() { name.to_lowercase() } else { key }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Name {}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
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

impl_value!([] Name, "name");

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
    /// `ValueError` if `name` is no name (`Name::new`).
    pub fn new(name: impl AsRef<str>, scope: Scope) -> Result<Self, ValueError> {
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
        assert_eq!(Name::new(" \t "), Err(ValueError::Empty { field: "name" }));
        assert_eq!(Name::new("a\u{7}b"), Err(ValueError::ControlCharacter { field: "name" }));
        assert_eq!(Name::new("x".repeat(MAX_NAME_CHARS + 1)), Err(ValueError::TooLong { field: "name", max_chars: MAX_NAME_CHARS }));
        assert_eq!(Name::optional(Some("  ")), Ok(None), "blank is none");
        assert_eq!(Name::new(" \t ").unwrap_err().to_string(), "the name must not be empty", "naming the field");
        assert!(Name::new("x".repeat(MAX_NAME_CHARS)).is_ok());
    }

    #[test]
    fn names_are_one_regardless_of_case_in_a_lookup_and_a_map() {
        assert_eq!(name("Jonathan"), name("jonathan"));
        assert!(name("Zoë") == "ZOË", "beyond ASCII, too");
        assert!(name("Zoë") == "zoe" && name("Straße") == "STRASSE" && name("Иван") == "ivan", "by their ASCII form");
        assert_eq!(name("Zoë").as_str(), "Zoë", "kept as written");
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
