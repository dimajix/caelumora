//! Texts measured in characters, not bytes: shortening one where it must stay short (issue 619).

/// `text` cut after `max_chars` characters, followed by `mark` where it was cut; as it is if it is
/// no longer.
pub fn shortened(text: &str, max_chars: usize, mark: &str) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}{mark}", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue 619: a text is cut after a number of characters - at a character's boundary, whatever
    /// its bytes -, marked where it was; one no longer stays as it is, marked nowhere.
    #[test]
    fn a_text_is_shortened_after_its_characters_and_marked_where_it_was_cut() {
        assert_eq!(shortened("Grüße", 3, "…"), "Grü…", "at a character's boundary");
        assert_eq!(shortened("Grüße", 5, "…"), "Grüße", "as long as allowed: as it is");
        assert_eq!(shortened("Grüße", 0, "…"), "…", "nothing of it allowed");
        assert_eq!(shortened("Grüße", 2, ""), "Gr", "unmarked");
        assert_eq!(shortened("", 2, "…"), "", "nothing to cut");
    }
}
