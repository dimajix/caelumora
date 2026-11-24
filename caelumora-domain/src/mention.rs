//! `@`-mentions: addressing a channel participant by its handle (see issue 29).
//!
//! A *handle* is a normalised form of a participant's name — transliterated to ASCII, lowercase,
//! with every run of characters that cannot appear in a mention collapsed into a single `-` — so
//! that "Sassy" is addressed as `@sassy`, "Dr. Who" as `@dr-who` and "Zoë" as `@zoe` (issue 177).
//! A mention typed with the original characters, `@zoë`, is normalised alike and addresses the same
//! participant. The group handles `@all` and `@channel` address every participant taking part where
//! the message is said - in its thread -, but its sender (issues 401, 594,
//! `Message::resolve_mentions`). Mentions are resolved to `ActorId`s once, when a message enters
//! the channel, and stored with the message - whom it mentions by name in `Message::mentions`, whom
//! it addresses in `Message::addressees` (issue 602); downstream, the text is parsed again only to
//! find which of its handles to highlight (`MentionDirectory::mentioned_spans`), never to decide
//! whom it addresses.

use crate::actor::ActorId;
use std::collections::HashMap;
use std::ops::Range;

/// The handles that address every participant at once, rather than one by its name: `@all` and its
/// synonym `@channel` (issue 401) - in a message, every participant taking part where it is said
/// (`MentionDirectory::resolve`). A participant whose name yields one of them is among those it
/// addresses, but is not addressed by it alone.
pub const GROUP_HANDLES: [&str; 2] = ["all", "channel"];

/// Returns whether `handle`, normalised as `handle_of` does, addresses every
/// participant (`GROUP_HANDLES`).
pub fn is_group_handle(handle: &str) -> bool {
    GROUP_HANDLES.contains(&handle)
}

/// Returns whether `c` may be part of a handle.
fn is_handle_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Returns whether an `@` after `prev` - the character before it, if any - may begin a mention: not
/// after a character of a handle or a `.`, which keeps email addresses from counting.
fn may_begin_mention(prev: Option<char>) -> bool {
    !prev.is_some_and(|p| is_handle_char(p) || p == '.')
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
struct MentionSpan {
    range: Range<usize>,
    handle: String,
}

/// Finds every mention in `text`, in order of occurrence.
///
/// A mention is an `@` followed by at least one handle character, where the
/// `@` is at the start of the text or not preceded by a handle character or
/// a `.` — which keeps email addresses like `sassy@example.com` from counting.
/// Trailing `-` are not part of the mention. Text inside backticks (inline
/// code and code fences) is ignored.
fn mention_spans(text: &str) -> Vec<MentionSpan> {
    let mut spans = Vec::new();
    let mut in_code = false;
    let mut prev: Option<char> = None;
    let mut chars = text.char_indices().peekable();

    while let Some((start, c)) = chars.next() {
        if c == '`' {
            in_code = !in_code;
        } else if c == '@' && !in_code && may_begin_mention(prev) {
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
                spans.push(MentionSpan {
                    range: start..end,
                    handle: handle_of(&text[start + 1..end]),
                });
            }
            prev = Some(last);
            continue;
        }
        prev = Some(c);
    }

    spans
}

/// Where the handle being typed in `text` up to `cursor` (a byte offset) begins (issue 579): the
/// offset of its `@`, if what stands before `cursor` is an `@` and characters of a handle, where a
/// mention may begin - as `mention_spans` finds them, outside code. `None` elsewhere.
pub fn handle_typed_at(text: &str, cursor: usize) -> Option<usize> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    if !typed.chars().all(is_handle_char) {
        return None;
    }
    // Inside code where an odd number of backticks stands before it, as `mention_spans` toggles.
    let in_code = before[..at].matches('`').count() % 2 == 1;
    (!in_code && may_begin_mention(before[..at].chars().next_back())).then_some(at)
}

/// Extracts the handles mentioned in `text`, normalised as `handle_of` does,
/// deduplicated and in order of first occurrence. See `mention_spans` for what counts as a
/// mention.
fn parse_mentions(text: &str) -> Vec<String> {
    let mut handles: Vec<String> = Vec::new();
    for span in mention_spans(text) {
        if !handles.contains(&span.handle) {
            handles.push(span.handle);
        }
    }
    handles
}

/// The handles of a channel's participants, used to resolve the mentions in
/// a message to the actors they address, and to find which of its handles
/// address whom it mentions.
#[derive(Debug, Clone, Default)]
pub struct MentionDirectory {
    handles: HashMap<String, Vec<ActorId>>,
    /// Every participant, in the order given: whom a group handle addresses.
    everyone: Vec<ActorId>,
}

impl MentionDirectory {
    /// Builds the directory from `(actor, name)` pairs. Participants whose
    /// name yields no handle cannot be mentioned by name, but are among
    /// those a group handle addresses.
    pub fn new<'a>(participants: impl IntoIterator<Item = (ActorId, &'a str)>) -> Self {
        let mut handles: HashMap<String, Vec<ActorId>> = HashMap::new();
        let mut everyone: Vec<ActorId> = Vec::new();
        for (id, name) in participants {
            if !everyone.contains(&id) {
                everyone.push(id);
            }
            let handle = handle_of(name);
            if !handle.is_empty() {
                let ids = handles.entry(handle).or_default();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Self { handles, everyone }
    }

    /// Returns the actors a handle can name in the channel: every participant, wherever they take
    /// part, if it is a group handle, else none if it is unknown, several if it is ambiguous. What
    /// a tool's questions ask by a handle, which go to the users of the channel wherever its call
    /// runs (`Questions::resolve`); a message is resolved where it is said (`resolve`).
    pub fn lookup(&self, handle: &str) -> &[ActorId] {
        if is_group_handle(handle) {
            return &self.everyone;
        }
        self.handles.get(handle).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Handles of participants that do not address one participant alone,
    /// sorted: those shared by more than one, which a mention addresses all
    /// of, and group handles a participant's name yields, which address
    /// everyone.
    pub fn ambiguous(&self) -> Vec<String> {
        let mut result: Vec<String> = self
            .handles
            .iter()
            .filter(|(handle, ids)| ids.len() > 1 || is_group_handle(handle))
            .map(|(handle, _)| handle.clone())
            .collect();
        result.sort();
        result
    }

    /// Resolves the mentions in `text`, said where `present` take part - in a thread, those taking
    /// part in it -, to the actors they address, each list deduplicated and in order of first
    /// mention: those mentioned by name, wherever they take part, and everyone addressed - those
    /// and the participants of `present`, whom a group handle addresses (issues 594, 602,
    /// `Message::mentions`, `Message::addressees`). Unknown handles are ignored — they stay plain
    /// text.
    pub fn resolve(&self, text: &str, present: &[ActorId]) -> (Vec<ActorId>, Vec<ActorId>) {
        let mut named: Vec<ActorId> = Vec::new();
        let mut addressed: Vec<ActorId> = Vec::new();
        for handle in parse_mentions(text) {
            let group = is_group_handle(&handle);
            for id in self.lookup(&handle) {
                if !group && !named.contains(id) {
                    named.push(*id);
                }
                if (!group || present.contains(id)) && !addressed.contains(id) {
                    addressed.push(*id);
                }
            }
        }
        (named, addressed)
    }

    /// The byte ranges of the mentions in `text` (`@` included, in the order they stand, as
    /// `mention_spans` finds them) whose handles address one of `addressees`, whom a message
    /// addresses (`Message::addressees`): what is highlighted of its words.
    pub fn mentioned_spans(&self, text: &str, addressees: &[ActorId]) -> Vec<Range<usize>> {
        mention_spans(text)
            .into_iter()
            .filter(|span| self.lookup(&span.handle).iter().any(|id| addressees.contains(id)))
            .map(|span| span.range)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::user::UserId;
    use std::collections::HashSet;

    /// Issue 579: a handle is being typed where an `@` and characters of a handle stand before the
    /// cursor, and a mention could begin there - not in an address, nor in code.
    #[test]
    fn a_handle_is_typed_where_a_mention_could_begin() {
        let typed = |text: &str| handle_typed_at(text, text.len());
        assert_eq!(typed("@"), Some(0));
        assert_eq!(typed("hi @sas"), Some(3));
        assert_eq!(typed("(@dr-w"), Some(1));
        assert_eq!(typed("hi @sassy "), None, "the word is done");
        assert_eq!(typed("mail me at bob@exa"), None, "an address");
        assert_eq!(typed("see `@co"), None, "in code");
        assert_eq!(typed("no mention"), None);
        assert_eq!(handle_typed_at("@bob and more", 4), Some(0), "up to the cursor");
    }

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

        let both = (vec![bob, sassy], vec![bob, sassy]);
        let by_name = directory.resolve("@bob, ask @SASSY and @nobody", &[]);
        assert_eq!(by_name, both, "by name, wherever they take part");
        assert_eq!(directory.resolve("no mentions here", &[bob, sassy]), (vec![], vec![]));
        assert!(directory.ambiguous().is_empty());
    }

    /// What is highlighted of a message's words are the handles that address someone it resolved
    /// its mentions to: a group handle where anyone taking part was (issue 401), none where nobody
    /// was or all have left.
    #[test]
    fn the_spans_mentioned_are_those_addressing_whom_a_message_mentions() {
        let sassy = ActorId::Agent(AgentId::new());
        let bob = ActorId::User(UserId::new());
        let directory = MentionDirectory::new(vec![(sassy, "Sassy"), (bob, "Bob")]);
        let text = "@sassy, @bob and @all, not @nobody";
        assert_eq!(directory.mentioned_spans(text, &[sassy]), vec![0..6, 17..21]);
        assert_eq!(directory.mentioned_spans(text, &[sassy, bob]), vec![0..6, 8..12, 17..21]);
        assert!(directory.mentioned_spans(text, &[]).is_empty(), "nobody mentioned");
        let gone = ActorId::User(UserId::new());
        assert!(directory.mentioned_spans(text, &[gone]).is_empty(), "nobody taking part");
    }

    #[test]
    fn a_name_beyond_ascii_is_mentioned_by_its_ascii_form_or_as_written() {
        let zoe = ActorId::User(UserId::new());
        let directory = MentionDirectory::new(vec![(zoe, "Zoë")]);
        assert_eq!(directory.resolve("@zoe, @Zoë and @ZOË", &[]), (vec![zoe], vec![zoe]));
    }

    /// A group handle addresses every participant taking part where it is said, but mentions
    /// nobody by name (issue 602).
    #[test]
    fn a_group_handle_addresses_every_participant_present() {
        let sassy = ActorId::Agent(AgentId::new());
        let bob = ActorId::User(UserId::new());
        let nameless = ActorId::Agent(AgentId::new());
        let directory =
            MentionDirectory::new(vec![(sassy, "Sassy"), (bob, "Bob"), (nameless, "!!!")]);

        let everyone = [sassy, bob, nameless];
        for text in ["@all, what do you think?", "@channel?", "@ALL", "@bob and @all"] {
            let (named, addressed) = directory.resolve(text, &everyone);
            let addressed: HashSet<ActorId> = addressed.into_iter().collect();
            assert_eq!(
                addressed,
                HashSet::from([sassy, bob, nameless]),
                "{text}: everyone, also who has no handle of their own"
            );
            let by_name = if text.contains("@bob") { vec![bob] } else { vec![] };
            assert_eq!(named, by_name, "{text}: by name only whom it names");
        }
        assert_eq!(directory.resolve("`@all`", &everyone), (vec![], vec![]), "not in code");
        let own = directory.resolve("@allison", &everyone);
        assert_eq!(own, (vec![], vec![]), "a handle of its own");
        assert!(directory.ambiguous().is_empty());
        assert!(
            is_group_handle("all") && is_group_handle("channel") && !is_group_handle("everyone")
        );
    }

    /// Issue 594: where not everyone takes part - in a thread -, a group handle addresses those who
    /// do alone, in the directory's order, and of them only participants it knows; whom it names
    /// it addresses wherever they take part.
    #[test]
    fn a_group_handle_addresses_only_who_takes_part_where_it_is_said() {
        let sassy = ActorId::Agent(AgentId::new());
        let bob = ActorId::User(UserId::new());
        let alice = ActorId::User(UserId::new());
        let directory =
            MentionDirectory::new(vec![(sassy, "Sassy"), (bob, "Bob"), (alice, "Alice")]);
        let unknown = ActorId::Agent(AgentId::new());
        let present = [alice, unknown, sassy];

        assert_eq!(directory.resolve("@all", &present), (vec![], vec![sassy, alice]));
        let named = directory.resolve("@all, and @bob", &present);
        assert_eq!(named, (vec![bob], vec![sassy, alice, bob]), "by name from outside too");
        assert_eq!(directory.resolve("@all", &[]), (vec![], vec![]), "nobody present");
    }

    #[test]
    fn a_participant_whose_handle_is_a_group_handle_is_addressed_with_everyone() {
        let all = ActorId::Agent(AgentId::new());
        let bob = ActorId::User(UserId::new());
        let directory = MentionDirectory::new(vec![(all, "All"), (bob, "Bob")]);

        assert_eq!(directory.lookup("all"), [all, bob], "the group wins");
        assert_eq!(
            directory.ambiguous(),
            vec!["all".to_string()],
            "and the handle is not the participant's alone"
        );
    }

    #[test]
    fn directory_resolves_ambiguous_handles_to_all_bearers() {
        let first = ActorId::Agent(AgentId::new());
        let second = ActorId::Agent(AgentId::new());
        let directory = MentionDirectory::new(vec![
            (first, "Sassy"),
            (second, "sassy"),
            (ActorId::None, "!!!"),
        ]);

        assert_eq!(directory.ambiguous(), vec!["sassy".to_string()]);
        let both = (vec![first, second], vec![first, second]);
        assert_eq!(directory.resolve("@sassy", &[]), both);
        assert!(directory.lookup("").is_empty());
    }
}
