//! `@`-mentions: addressing a channel participant by its handle (see issue 29).
//!
//! A *handle* is a normalised form of a participant's name — transliterated
//! to ASCII, lowercase, with every run of characters that cannot appear in a
//! mention collapsed into a single `-` — so that "Sassy" is addressed as
//! `@sassy`, "Dr. Who" as `@dr-who` and "Zoë" as `@zoe` (issue 177). A
//! mention typed with the original characters, `@zoë`, is normalised alike
//! and addresses the same participant. Mentions are resolved to `ActorId`s once, when a message enters
//! the channel, and stored with the message in `Message::mentions`; nothing
//! downstream re-parses the text.

use crate::actor::ActorId;
use std::collections::HashMap;
use std::ops::Range;

/// Returns whether `c` may be part of a handle.
fn is_handle_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Derives the handle under which a participant called `name` can be
/// mentioned: its ASCII form (`any_ascii`, as `Name` compares names; the web
/// UI uses the npm package of the same name), lowercase, every run of other
/// characters one `-`. Returns an empty string if nothing of it may appear
/// in a handle, in which case the participant cannot be mentioned.
pub fn handle_of(name: &str) -> String {
    let ascii = any_ascii::any_ascii(name);
    let mut handle = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        if is_handle_char(c) {
            handle.extend(c.to_lowercase());
        } else if !handle.ends_with('-') {
            handle.push('-');
        }
    }
    handle.trim_matches('-').to_string()
}

/// A mention found in a text: the byte range of `@handle` (including the
/// `@`) and the normalised handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionSpan {
    pub range: Range<usize>,
    pub handle: String,
}

/// Finds every mention in `text`, in order of occurrence.
///
/// A mention is an `@` followed by at least one handle character, where the
/// `@` is at the start of the text or not preceded by a handle character or
/// a `.` — which keeps email addresses like `sassy@example.com` from counting.
/// Trailing `-` are not part of the mention. Text inside backticks (inline
/// code and code fences) is ignored.
pub fn mention_spans(text: &str) -> Vec<MentionSpan> {
    let mut spans = Vec::new();
    let mut in_code = false;
    let mut prev: Option<char> = None;
    let mut chars = text.char_indices().peekable();

    while let Some((start, c)) = chars.next() {
        if c == '`' {
            in_code = !in_code;
        } else if c == '@' && !in_code && !prev.is_some_and(|p| is_handle_char(p) || p == '.') {
            let mut end = start + c.len_utf8();
            let mut last = c;
            while let Some(&(i, n)) = chars.peek() {
                if !is_handle_char(n) {
                    break;
                }
                chars.next();
                last = n;
                if n != '-' {
                    end = i + n.len_utf8();
                }
            }
            if end > start + c.len_utf8() {
                spans.push(MentionSpan { range: start..end, handle: handle_of(&text[start + 1..end]) });
            }
            prev = Some(last);
            continue;
        }
        prev = Some(c);
    }

    spans
}

/// Extracts the handles mentioned in `text`, normalised as `handle_of` does,
/// deduplicated and in order of first occurrence. See `mention_spans` for what counts as a
/// mention.
pub fn parse_mentions(text: &str) -> Vec<String> {
    let mut handles: Vec<String> = Vec::new();
    for span in mention_spans(text) {
        if !handles.contains(&span.handle) {
            handles.push(span.handle);
        }
    }
    handles
}

/// The handles of a channel's participants, used to resolve the mentions in
/// a message to the actors they address.
#[derive(Debug, Clone, Default)]
pub struct MentionDirectory {
    handles: HashMap<String, Vec<ActorId>>,
}

impl MentionDirectory {
    /// Builds the directory from `(actor, name)` pairs. Participants whose
    /// name yields no handle are left out.
    pub fn new<'a>(participants: impl IntoIterator<Item = (ActorId, &'a str)>) -> Self {
        let mut handles: HashMap<String, Vec<ActorId>> = HashMap::new();
        for (id, name) in participants {
            let handle = handle_of(name);
            if !handle.is_empty() {
                let ids = handles.entry(handle).or_default();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Self { handles }
    }

    /// Returns the actors a handle addresses: none if it is unknown, several
    /// if it is ambiguous.
    pub fn lookup(&self, handle: &str) -> &[ActorId] {
        self.handles.get(handle).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Handles shared by more than one participant, sorted. A mention of such
    /// a handle addresses all of them.
    pub fn ambiguous(&self) -> Vec<String> {
        let mut result: Vec<String> = self.handles.iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(handle, _)| handle.clone())
            .collect();
        result.sort();
        result
    }

    /// Resolves the mentions in `text` to the actors they address,
    /// deduplicated and in order of first mention. Unknown handles are
    /// ignored — they stay plain text.
    pub fn resolve(&self, text: &str) -> Vec<ActorId> {
        let mut result: Vec<ActorId> = Vec::new();
        for handle in parse_mentions(text) {
            for id in self.lookup(&handle) {
                if !result.contains(id) {
                    result.push(*id);
                }
            }
        }
        result
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::user::UserId;

    #[test]
    fn handle_of_lowercases_and_collapses_separators() {
        assert_eq!(handle_of("Sassy"), "sassy");
        assert_eq!(handle_of("Dr. Who"), "dr-who");
        assert_eq!(handle_of("  Mr   Smith  "), "mr-smith");
        assert_eq!(handle_of("data_bot-2"), "data_bot-2");
        assert_eq!(handle_of("Zoë"), "zoe", "by its ASCII form");
        assert_eq!(handle_of("Иван Петров"), "ivan-petrov");
        assert_eq!(handle_of("!!!"), "");
    }

    #[test]
    fn parse_mentions_finds_handles_in_order_without_duplicates() {
        assert_eq!(
            parse_mentions("@Sassy and @bob, what do you think? @sassy?"),
            vec!["sassy".to_string(), "bob".to_string()]
        );
        assert_eq!(parse_mentions("(@dr-who)."), vec!["dr-who".to_string()]);
        assert_eq!(parse_mentions("ask @sassy- please"), vec!["sassy".to_string()]);
    }

    #[test]
    fn mention_spans_cover_the_mention_without_trailing_dashes() {
        let text = "Hi @Zoë- and @bob!";
        let spans = mention_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].range.clone()], "@Zoë");
        assert_eq!(spans[0].handle, "zoe", "normalised as a handle: the text keeps what was typed");
        assert_eq!(&text[spans[1].range.clone()], "@bob");
    }

    #[test]
    fn parse_mentions_ignores_emails_bare_at_and_code() {
        assert!(parse_mentions("mail sassy@example.com").is_empty());
        assert!(parse_mentions("first.@sassy").is_empty());
        assert!(parse_mentions("just an @ sign").is_empty());
        assert!(parse_mentions("use `@sassy` literally").is_empty());
        assert!(parse_mentions("```\n@sassy\n```").is_empty());
        assert_eq!(parse_mentions("`code` then @bob"), vec!["bob".to_string()]);
    }

    #[test]
    fn directory_resolves_known_handles_and_ignores_unknown_ones() {
        let sassy = ActorId::Agent(AgentId::new());
        let bob = ActorId::User(UserId::new());
        let directory = MentionDirectory::new(vec![(sassy, "Sassy"), (bob, "Bob")]);

        assert_eq!(directory.resolve("@bob, ask @SASSY and @nobody"), vec![bob, sassy]);
        assert!(directory.resolve("no mentions here").is_empty());
        assert!(directory.ambiguous().is_empty());
    }

    #[test]
    fn a_name_beyond_ascii_is_mentioned_by_its_ascii_form_or_as_written() {
        let zoe = ActorId::User(UserId::new());
        let directory = MentionDirectory::new(vec![(zoe, "Zoë")]);
        assert_eq!(directory.resolve("@zoe, @Zoë and @ZOË"), vec![zoe]);
    }

    #[test]
    fn directory_resolves_ambiguous_handles_to_all_bearers() {
        let first = ActorId::Agent(AgentId::new());
        let second = ActorId::Agent(AgentId::new());
        let directory = MentionDirectory::new(vec![(first, "Sassy"), (second, "sassy"), (ActorId::None, "!!!")]);

        assert_eq!(directory.ambiguous(), vec!["sassy".to_string()]);
        assert_eq!(directory.resolve("@sassy"), vec![first, second]);
        assert!(directory.lookup("").is_empty());
    }
}
