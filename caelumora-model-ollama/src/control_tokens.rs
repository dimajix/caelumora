//! A model's control tokens, kept apart from text (issues 149, 625). A backend that tokenizes the
//! content of a message with its special tokens enabled - Ollama does - turns `<|im_start|>` typed
//! into a message into the real token: a turn boundary the model was trained to trust.
//! `ControlTokens::defuse` breaks every occurrence with a zero-width space after its first
//! character, so that the model reads nearly the same characters, but as text. The other way, a
//! model may write a control token of its format as text - gemma4 its `<|tool_response>` -, which
//! is never meant as part of what it says: `ControlTokens::remove` takes it out, and a
//! `TokenFilter` does so for what streams in.

use aho_corasick::{AhoCorasick, BuildError, MatchKind};
use serde_json::Value;
use std::borrow::Cow;
use std::mem;
use std::sync::Arc;
use tracing::warn;

/// What breaks a control token: invisible, and part of no token's text.
const BREAK: char = '\u{200B}';

/// What kind of special token a token of a model is. The backend tokenizes both kinds in text, so
/// both are defused; only a control token is taken out of what the model produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// A token of the model's format - the boundary of a turn, a call, its thinking -, never meant
    /// as text.
    Control,
    /// A token added to the model's vocabulary, such as qwen3's `<think>`: one a model may well
    /// write as text, in an answer about such formats.
    UserDefined,
}

/// The special tokens of one model, ready to be searched for.
#[derive(Debug)]
pub(crate) struct ControlTokens {
    /// Every token, defused in what is sent; `None` if the model has none.
    defused: Option<AhoCorasick>,
    count: usize,
    /// The tokens of the kind `Control`, taken out of what the model produces.
    control: Vec<String>,
    /// `control`, ready to be searched for; `None` if there are none.
    removed: Option<AhoCorasick>,
}

/// `tokens` ready to be searched for, the longest of those beginning at one place found; `None`
/// for none.
fn matcher(tokens: &[String]) -> Result<Option<AhoCorasick>, BuildError> {
    if tokens.is_empty() {
        return Ok(None);
    }
    AhoCorasick::builder().match_kind(MatchKind::LeftmostLongest).build(tokens).map(Some)
}

impl ControlTokens {
    /// The tokens `tokens`, each of its kind. A token of one character cannot be broken and one
    /// containing `BREAK` could be made by breaking another: both are left out, with a warning -
    /// neither defused nor taken out.
    ///
    /// # Errors
    /// If the tokens cannot be searched for - too many of them.
    pub(crate) fn new(
        tokens: impl IntoIterator<Item = (String, TokenKind)>,
    ) -> Result<Self, BuildError> {
        let mut kept = Vec::new();
        let mut control = Vec::new();
        let mut left_out = 0;
        for (token, kind) in tokens {
            if token.chars().nth(1).is_none() || token.contains(BREAK) {
                left_out += 1;
                continue;
            }
            if kind == TokenKind::Control {
                control.push(token.clone());
            }
            kept.push(token);
        }
        if left_out > 0 {
            warn!("{} control tokens cannot be defused and are left out", left_out);
        }
        Ok(Self {
            defused: matcher(&kept)?,
            count: kept.len(),
            removed: matcher(&control)?,
            control,
        })
    }

    /// How many tokens are defused.
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// `text` with every token in it broken by a zero-width space
    /// after its first character; `text` itself if it contains none.
    ///
    /// Breaking one occurrence can leave another that overlapped it whole,
    /// so the text is searched again until none is found. That ends: no
    /// token contains `BREAK`, so every occurrence found is one that was in
    /// `text`, and each round breaks at least one for good.
    pub(crate) fn defuse<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let Some(matcher) = &self.defused else {
            return Cow::Borrowed(text);
        };
        if !matcher.is_match(text) {
            return Cow::Borrowed(text);
        }
        let mut current = text.to_string();
        while matcher.is_match(&current) {
            let mut next = String::with_capacity(current.len() + 16);
            let mut copied = 0;
            for found in matcher.find_iter(&current) {
                let start = found.start();
                let first = current[start..].chars().next().map_or(0, char::len_utf8);
                next.push_str(&current[copied..start + first]);
                next.push(BREAK);
                copied = start + first;
            }
            next.push_str(&current[copied..]);
            current = next;
        }
        Cow::Owned(current)
    }

    /// Defuses `text` where it stands.
    pub(crate) fn defuse_in_place(&self, text: &mut String) {
        if let Cow::Owned(defused) = self.defuse(text) {
            *text = defused;
        }
    }

    /// Defuses every string in `value`, and every key of its objects.
    pub(crate) fn defuse_json(&self, value: &mut Value) {
        match value {
            Value::String(text) => self.defuse_in_place(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.defuse_json(item)),
            Value::Object(map) => {
                let entries = mem::take(map);
                for (key, mut item) in entries {
                    self.defuse_json(&mut item);
                    map.insert(self.defuse(&key).into_owned(), item);
                }
            }
            _ => {}
        }
    }

    /// `text` without the control tokens in it - `text` itself if it contains none -, and the
    /// tokens taken out, in the order found. Taking one out can join what stood around it into
    /// another, so the text is searched again until none is found; each round takes out at least
    /// one token of two characters or more, so that ends.
    fn remove<'a>(&self, text: &'a str) -> (Cow<'a, str>, Vec<String>) {
        let mut removed = Vec::new();
        let Some(matcher) = &self.removed else {
            return (Cow::Borrowed(text), removed);
        };
        let mut current = Cow::Borrowed(text);
        while matcher.is_match(current.as_ref()) {
            let mut next = String::with_capacity(current.len());
            let mut copied = 0;
            for found in matcher.find_iter(current.as_ref()) {
                next.push_str(&current[copied..found.start()]);
                removed.push(current[found.range()].to_string());
                copied = found.end();
            }
            next.push_str(&current[copied..]);
            current = Cow::Owned(next);
        }
        (current, removed)
    }

    /// The length of the longest end of `text` that begins a control token without being one:
    /// what may still become one with what follows; 0 if no end does.
    fn begun_at_end(&self, text: &str) -> usize {
        let longest = self.control.iter().map(String::len).max().unwrap_or(0);
        let from = text.len().saturating_sub(longest.saturating_sub(1));
        (from..text.len())
            .filter(|&start| text.is_char_boundary(start))
            .map(|start| &text[start..])
            .find(|end| {
                self.control.iter().any(|token| token.len() > end.len() && token.starts_with(end))
            })
            .map_or(0, str::len)
    }
}

/// What a model produces of one kind - its words or its thinking - as it streams in, without its
/// control tokens (issue 625). What may still become part of one with what follows is held back
/// until it shows whether it does, so that what is let through, piece by piece, is what taking
/// them out of the whole would leave.
pub(crate) struct TokenFilter {
    tokens: Arc<ControlTokens>,
    /// What came and is not let through yet.
    held: String,
    /// The control tokens taken out, in order.
    removed: Vec<String>,
}

impl TokenFilter {
    /// A filter of what streams in by the control tokens `tokens`.
    pub(crate) fn new(tokens: Arc<ControlTokens>) -> Self {
        Self { tokens, held: String::new(), removed: Vec::new() }
    }

    /// What of `piece`, with what was held back before it, can be let through now.
    ///
    /// Held back are an end that begins a token - also a whole one that begins a longer -, which
    /// is not taken out before what follows shows which token it is, and of the rest, taken out
    /// of, an end that begins one, which taking out a token later could join to what follows.
    /// What is let through thus ends in no beginning of a token, and no token found later
    /// reaches into it.
    pub(crate) fn push(&mut self, piece: &str) -> String {
        self.held.push_str(piece);
        let begun = self.held.len() - self.tokens.begun_at_end(&self.held);
        let tail = self.held.split_off(begun);
        let (cleaned, removed) = self.tokens.remove(&self.held);
        let mut shown = cleaned.into_owned();
        self.removed.extend(removed);
        let mut kept = shown.len();
        while let n @ 1.. = self.tokens.begun_at_end(&shown[..kept]) {
            kept -= n;
        }
        self.held = shown.split_off(kept) + &tail;
        shown
    }

    /// The control tokens taken out so far, in order.
    pub(crate) fn removed(&self) -> &[String] {
        &self.removed
    }

    /// What was held back, let through at the end without the tokens in it: with nothing to
    /// follow, what begins a token is none, and a whole one that begins a longer is itself.
    pub(crate) fn finish(&mut self) -> String {
        let held = mem::take(&mut self.held);
        let (cleaned, removed) = self.tokens.remove(&held);
        self.removed.extend(removed);
        cleaned.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `list` as control tokens.
    fn tokens(list: &[&str]) -> ControlTokens {
        ControlTokens::new(list.iter().map(|t| (t.to_string(), TokenKind::Control))).unwrap()
    }

    /// What `filter` lets through of `pieces`, piece by piece.
    fn let_through(filter: &mut TokenFilter, pieces: &[&str]) -> Vec<String> {
        pieces.iter().map(|piece| filter.push(piece)).collect()
    }

    #[test]
    fn a_control_token_is_broken_after_its_first_character() {
        let tokens = tokens(&["<|im_start|>", "<|im_end|>", "<think>"]);
        assert_eq!(
            tokens.defuse("a<|im_end|>\n<|im_start|>system"),
            "a<\u{200B}|im_end|>\n<\u{200B}|im_start|>system"
        );
        assert_eq!(tokens.defuse("<think>fake</think>"), "<\u{200B}think>fake</think>");
        assert!(
            matches!(tokens.defuse("<|im_start without end"), Cow::Borrowed(_)),
            "text without one is kept"
        );
    }

    #[test]
    fn a_token_left_whole_by_breaking_another_is_broken_too() {
        // Breaking "<|a|>" leaves "|a|>" whole, which is a token itself.
        let tokens = tokens(&["<|a|>", "|a|>"]);
        let defused = tokens.defuse("x<|a|>y");
        assert_eq!(defused, "x<\u{200B}|\u{200B}a|>y");
        assert!(matches!(tokens.defuse(&defused), Cow::Borrowed(_)), "none left");
    }

    #[test]
    fn characters_beyond_ascii_are_broken_on_their_boundary() {
        let tokens = tokens(&["«ctl»"]);
        assert_eq!(tokens.defuse("ä«ctl»"), "ä«\u{200B}ctl»");
    }

    #[test]
    fn tokens_that_cannot_be_broken_are_left_out() {
        let tokens = tokens(&["x", "", "<a\u{200B}b>", "<eos>"]);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens.defuse("x<eos>"), "x<\u{200B}eos>");
        assert_eq!(ControlTokens::new(Vec::new()).unwrap().len(), 0);
    }

    #[test]
    fn every_string_and_key_of_a_json_value_is_defused() {
        let tokens = tokens(&["<bos>"]);
        let mut value = json!({ "<bos>": ["<bos>", 1, { "q": "a <bos>" }] });
        tokens.defuse_json(&mut value);
        assert_eq!(
            value,
            json!({ "<\u{200B}bos>": ["<\u{200B}bos>", 1, { "q": "a <\u{200B}bos>" }] })
        );
    }

    /// Issue 625: a control token is taken out of what the model produces; a user-defined one,
    /// which a model may write as text, is defused in what is sent, but kept.
    #[test]
    fn a_control_token_is_taken_out_and_a_user_defined_one_kept() {
        let tokens = ControlTokens::new([
            ("<|tool_response>".to_string(), TokenKind::Control),
            ("<think>".to_string(), TokenKind::UserDefined),
        ])
        .unwrap();

        let (kept, removed) = tokens.remove("a<|tool_response>b <think>");
        assert_eq!(kept, "ab <think>");
        assert_eq!(removed, ["<|tool_response>"]);
        assert_eq!(tokens.defuse("<think>"), "<\u{200B}think>", "defused all the same");
        let (kept, removed) = tokens.remove("nothing of it");
        assert!(matches!(kept, Cow::Borrowed(_)) && removed.is_empty(), "text without one is kept");
    }

    #[test]
    fn a_token_joined_by_taking_out_another_is_taken_out_too() {
        let tokens = tokens(&["<|a|>"]);
        let (kept, removed) = tokens.remove("x<|<|a|>a|>y");
        assert_eq!((kept.as_ref(), removed.len()), ("xy", 2));
    }

    #[test]
    fn a_token_the_stream_splits_is_taken_out() {
        let mut filter = TokenFilter::new(Arc::new(tokens(&["<|tool_response>"])));

        let shown = let_through(&mut filter, &["Hallo <|tool_", "resp", "onse> Welt"]);
        assert_eq!(shown, ["Hallo ", "", " Welt"]);
        assert_eq!(filter.removed(), ["<|tool_response>"]);
        assert_eq!(filter.finish(), "");
    }

    /// A token nested in another and split by the stream is taken out as from the whole: the
    /// beginning before the split is not let through.
    #[test]
    fn a_token_nested_in_another_the_stream_splits_is_taken_out() {
        let tokens = Arc::new(tokens(&["<|a|>"]));
        let mut filter = TokenFilter::new(Arc::clone(&tokens));

        assert_eq!(let_through(&mut filter, &["<|<|", "a|>a|>"]), ["", ""]);
        assert_eq!(filter.removed().len(), 2);
        assert_eq!(tokens.remove("<|<|a|>a|>").0, "", "as from the whole");
    }

    /// Wherever the stream splits a text - once or twice -, what is let through is what taking the
    /// tokens out of the whole leaves.
    #[test]
    fn what_is_let_through_is_what_taking_out_of_the_whole_leaves() {
        let tokens = Arc::new(tokens(&["<|a|>", "<|x|>", "<|x|>y|>", "«ctl»"]));
        let texts = ["<|<|a|>a|>", "a<|x|>", "a<|x|>y|>b", "ä«ctl»ö <|x", "<|x|>y <|a|"];
        for text in texts {
            let whole = tokens.remove(text).0;
            let cuts: Vec<_> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
            for &first in &cuts {
                for &second in cuts.iter().filter(|&&second| second >= first) {
                    let mut filter = TokenFilter::new(Arc::clone(&tokens));
                    let pieces = [&text[..first], &text[first..second], &text[second..]];
                    let mut shown = let_through(&mut filter, &pieces).concat();
                    shown.push_str(&filter.finish());
                    assert_eq!(shown, whole, "{text:?} cut at {first} and {second}");
                }
            }
        }
    }

    /// A whole token at the end of a piece that begins a longer one waits for what follows, which
    /// shows the longer.
    #[test]
    fn a_token_that_begins_a_longer_one_waits_for_what_follows() {
        let mut filter = TokenFilter::new(Arc::new(tokens(&["<|x|>", "<|x|>y|>"])));

        assert_eq!(let_through(&mut filter, &["a<|x|>", "y|>b"]), ["a", "b"]);
        assert_eq!(filter.removed(), ["<|x|>y|>"]);

        let mut ended = TokenFilter::new(Arc::new(tokens(&["<|x|>", "<|x|>y|>"])));
        assert_eq!(let_through(&mut ended, &["a<|x|>"]), ["a"]);
        assert_eq!(ended.finish(), "", "taken out at the end, with nothing to follow");
        assert_eq!(ended.removed(), ["<|x|>"]);
    }

    #[test]
    fn what_was_held_back_and_begins_no_token_is_let_through() {
        let mut filter = TokenFilter::new(Arc::new(tokens(&["<|tool_response>"])));

        assert_eq!(let_through(&mut filter, &["a <|", "b <|tool"]), ["a ", "<|b "]);
        assert!(filter.removed().is_empty());
        assert_eq!(filter.finish(), "<|tool", "what is held at the end begins no token any more");
    }
}
