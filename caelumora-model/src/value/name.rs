//! Names that things are looked up by (issue 177): `Name`, and `ScopedName`
//! for what is found by name within its scope - and a session's, which it
//! could be looked up by (issue 248). A user's name, though a user provider
//! such as a directory decides it, is one, too (issue 250): it is an
//! identity, and two that differ only as names do not are one.

use super::text::{Text, TextRule};
use super::ValueError;
use crate::mention::handle_of;
use crate::scope::Scope;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

/// The longest name, in characters: a name stands in prompts and lists.
pub const MAX_NAME_CHARS: usize = 100;

/// What a name may be, as a text: one line of at most `MAX_NAME_CHARS`.
pub enum NameRule {}

impl TextRule for NameRule {
    const FIELD: &'static str = "name";
    const MAX_CHARS: usize = MAX_NAME_CHARS;
    const ONE_LINE: bool = true;
}

/// A name that something is looked up by: an agent's, a personality's, a
/// session template's, a channel's, a session's, a user's. A text of its own rule
/// (`NameRule`), kept exactly as written, and compared by its ASCII form
/// regardless of case and of how whitespace runs (`fold`): "Jonathan",
/// "jonathan" and "JONATHAN" are one name, and so are "Zoë" and "Zoe" and
/// "Dr.  Who" and "Dr. Who", in a lookup, in a map and as a key. Written
/// down as a string, and checked when read.
#[derive(Clone)]
pub struct Name {
    text: Text<NameRule>,
    /// What it is compared and hashed by, made once with the name: a name
    /// never changes.
    key: String,
}

impl Name {
    /// `text` as a name, as it is written.
    ///
    /// # Errors
    /// `ValueError` if it is no text of the rule `NameRule`: empty, of
    /// whitespace alone, more than one line, with a control character, or
    /// longer than `MAX_NAME_CHARS`.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = Text::<NameRule>::new(text)?;
        let key = fold(text.as_str());
        Ok(Self { text, key })
    }

    /// A name the code itself gives, such as every approver's
    /// (`ActorId::approver_name`): taken unchecked, so that it needs no
    /// error that cannot happen; each is checked by a test instead.
    pub(crate) fn fixed(text: &'static str) -> Self {
        Self { text: Text::fixed(text), key: fold(text) }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    /// The handle the name is `@`-mentioned by (see `crate::mention`);
    /// empty if it has no character a handle may have.
    pub fn handle(&self) -> String {
        handle_of(self.as_str())
    }
}

/// What a name is compared by: transliterated to ASCII (`any_ascii` - the
/// web UI uses the npm package of the same name), in lower case, every run
/// of whitespace one space, none at its ends. A name of which nothing is
/// left so is compared by its lower case, its whitespace so, too.
pub(crate) fn fold(name: &str) -> String {
    let tidied = |text: String| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let key = tidied(any_ascii::any_ascii(name).to_lowercase());
    if key.is_empty() { tidied(name.to_lowercase()) } else { key }
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

    /// What it is compared by, as one text (issue 447): its scope and its
    /// name as names are compared (`fold`), apart by a line break, which no
    /// name holds. Two scoped names are equal exactly if their keys are - so
    /// that a store keeps names unique by their keys, a database by a unique
    /// index. The key follows how names are folded: should that change, keys
    /// stored before are stale.
    pub fn key(&self) -> String {
        format!("{}\n{}", self.scope.key(), self.name.key)
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
    use crate::session::SessionId;
    use crate::project::ProjectId;
    use crate::user::UserId;
    use std::collections::HashSet;

    /// The key of a scoped name is equal exactly where the names are (issue
    /// 447): as names are compared, and only in one scope.
    #[test]
    fn two_scoped_names_have_one_key_exactly_if_they_are_equal() {
        let (session, user) = (SessionId::new(), UserId::new());
        let names = ["Zoë", "zoe", "Dr.  Who", "dr. who", "Other"];
        // A project's id equal to the session's, so that the kind of scope alone tells their
        // keys apart.
        let project = ProjectId(session.0);
        let scopes = [Scope::GlobalScope, Scope::SessionScope { session_id: session }, Scope::ProjectScope { project_id: project }, Scope::UserScope { user_id: user }];
        let all: Vec<ScopedName> = scopes.iter().flat_map(|scope| names.iter().map(|name| ScopedName::new(name, scope.clone()).unwrap())).collect();
        for left in &all {
            for right in &all {
                assert_eq!(left.key() == right.key(), left == right, "{left:?} and {right:?}");
            }
        }
    }

    fn name(text: &str) -> Name {
        Name::new(text).expect("a name")
    }

    #[test]
    fn a_name_is_kept_exactly_as_written() {
        assert_eq!(name("  Dr.   Who ").as_str(), "  Dr.   Who ", "checked, not changed");
        assert_eq!(name("  Dr.   Who "), name("Dr. Who"), "but compared regardless of how whitespace runs");
        assert_eq!(Name::new(""), Err(ValueError::Empty { field: "name" }));
        assert_eq!(Name::new(" \t "), Err(ValueError::Blank { field: "name" }));
        assert_eq!(Name::new("Dr.\nWho"), Err(ValueError::LineBreak { field: "name" }));
        assert_eq!(Name::new("a\u{7}b"), Err(ValueError::ControlCharacter { field: "name" }));
        assert_eq!(Name::new("x".repeat(MAX_NAME_CHARS + 1)), Err(ValueError::TooLong { field: "name", max_chars: MAX_NAME_CHARS }));
        assert_eq!(Name::optional(Some("")), Ok(None), "empty is none");
        assert_eq!(Name::optional(Some("  ")), Err(ValueError::Blank { field: "name" }), "whitespace is no name");
        assert_eq!(Name::new("").unwrap_err().to_string(), "the name must not be empty", "naming the field");
        assert!(Name::new("x".repeat(MAX_NAME_CHARS)).is_ok());
    }

    #[test]
    fn names_are_one_regardless_of_case_in_a_lookup_and_a_map() {
        assert_eq!(name("Jonathan"), name("jonathan"));
        assert!(name("Zoë") == "ZOË", "beyond ASCII, too");
        assert!(name("Zoë") == "zoe" && name("Straße") == "STRASSE" && name("Иван") == "ivan", "by their ASCII form");
        assert_eq!(name("Zoë").as_str(), "Zoë", "kept as written");
        assert!(name("Jonathan") != "", "a text that is no name is none");
        let set: HashSet<Name> = [name("Sassy"), name("sassy")].into_iter().collect();
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
