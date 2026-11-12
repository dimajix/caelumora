//! What a command is called (issue 560): `CommandName`, its words from its group to itself - a
//! variant of the identifiers.

use super::ValueError;

/// What a command name looks like, in the error for one that does not.
const EXPECTED: &str = "words of lower case letters, digits and '-', separated by single spaces";

/// The name of a command: its words, from its group to itself (`channel list`), each of lower case
/// letters, digits and `-`, separated by single spaces - so that it is written one way only, and
/// compared as written. A line typed begins with it if its words begin the line, whatever
/// whitespace runs between them there (`strip_from`). Written down as the string of its words,
/// and checked when read.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandName(String);

impl CommandName {
    /// `text` as a command name, as it is written.
    ///
    /// # Errors
    /// `ValueError` if it is empty, or anything but words of the characters above separated by
    /// single spaces.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref();
        if text.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        let word = |word: &str| {
            !word.is_empty()
                && word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        if !text.split(' ').all(word) {
            return Err(ValueError::Malformed { field: Self::FIELD, expected: EXPECTED });
        }
        Ok(Self(text.to_string()))
    }

    /// A name the code itself gives, such as `/help`'s: taken unchecked, so that it needs no
    /// error that cannot happen. The commands of a set are checked as the set is made
    /// (`caelumora_api::command::CommandSet::new`).
    pub fn fixed(text: &'static str) -> Self {
        Self(text.to_string())
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `other` begins with this name, word by word - itself too: `channel` begins
    /// `channel list`, `chan` begins neither.
    pub fn begins(&self, other: &CommandName) -> bool {
        let mut words = other.0.split(' ');
        self.0.split(' ').all(|word| words.next() == Some(word))
    }

    /// The rest of `line` - the arguments of the command, trimmed - if `line` begins with this
    /// name word by word, its words separated by any whitespace there: `/channel   list all`
    /// begins with `channel list` and leaves `all`.
    pub fn strip_from<'a>(&self, line: &'a str) -> Option<&'a str> {
        let mut rest = line.trim_start();
        for word in self.0.split(' ') {
            let after = rest.strip_prefix(word)?;
            if after.starts_with(|c: char| !c.is_whitespace()) {
                return None;
            }
            rest = after.trim_start();
        }
        Some(rest.trim_end())
    }
}

impl_value!([] CommandName, "command name");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_name_is_words_separated_by_single_spaces() {
        for name in ["help", "channel list", "create-agent", "agent create 2"] {
            assert_eq!(CommandName::new(name).unwrap().as_str(), name);
        }
        assert!(matches!(CommandName::new(""), Err(ValueError::Empty { .. })));
        for name in ["Help", "channel  list", " help", "help ", "channel\tlist", "/help", "zäh"] {
            assert!(matches!(CommandName::new(name), Err(ValueError::Malformed { .. })), "{name}");
        }
        let read: CommandName = serde_json::from_value(serde_json::json!("channel list")).unwrap();
        assert_eq!(read.as_str(), "channel list");
        assert!(serde_json::from_value::<CommandName>(serde_json::json!("channel  list")).is_err());
    }

    #[test]
    fn a_name_begins_the_names_its_words_begin() {
        let name = |text: &str| CommandName::new(text).unwrap();
        assert!(name("channel").begins(&name("channel list")));
        assert!(name("channel list").begins(&name("channel list")));
        assert!(!name("channel list").begins(&name("channel")));
        assert!(!name("chan").begins(&name("channel list")));
        assert!(!name("channel li").begins(&name("channel list")));
    }

    #[test]
    fn a_line_beginning_with_the_name_leaves_its_arguments() {
        let name = CommandName::new("channel list").unwrap();
        assert_eq!(name.strip_from("channel list"), Some(""));
        assert_eq!(name.strip_from("channel   list  all of them "), Some("all of them"));
        assert_eq!(name.strip_from("channel\tlist\nall"), Some("all"));
        assert_eq!(name.strip_from("channel lists"), None);
        assert_eq!(name.strip_from("channel"), None);
        assert_eq!(name.strip_from("channels list"), None);
        assert_eq!(CommandName::new("help").unwrap().strip_from("help wakeup"), Some("wakeup"));
    }
}
