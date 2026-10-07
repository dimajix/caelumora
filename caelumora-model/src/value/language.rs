//! The language a conversation is held in (issue 56): `Language`, a BCP 47
//! tag such as `de` or `en-GB`. A user prefers one, a channel is held in
//! one, and the agents of the channel are told which - nothing is
//! translated.

use super::ValueError;

/// The longest language tag, in characters: longer than any a user would
/// choose, short enough for a prompt.
pub const MAX_LANGUAGE_CHARS: usize = 35;

/// What a language tag looks like, in the error for one that does not.
const EXPECTED: &str = "a tag such as \"de\" or \"en-GB\"";

/// A language as a BCP 47 tag: a primary language of two or three letters,
/// followed by subtags of one to eight letters or digits - a script, a
/// region, a variant. Only its form is checked, not whether the language
/// exists: models know the tags, and a list of them would always be
/// incomplete. Kept in the usual case - `de-CH`, `zh-Hant-TW`, `sr-Latn` -
/// so that one language is one tag; `_` is taken as `-`. Written down as a
/// string, and checked when read.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Language(String);

impl Language {
    /// `tag` as a language, trimmed and in the usual case.
    ///
    /// # Errors
    /// `ValueError` if it is longer than `MAX_LANGUAGE_CHARS` or no tag of
    /// the form above.
    pub fn new(tag: impl AsRef<str>) -> Result<Self, ValueError> {
        let tag = tag.as_ref().trim();
        if tag.chars().count() > MAX_LANGUAGE_CHARS {
            return Err(ValueError::TooLong { field: Self::FIELD, max_chars: MAX_LANGUAGE_CHARS });
        }
        let mut subtags = tag.split(['-', '_']);
        let primary = subtags.next().unwrap_or_default();
        if !(2..=3).contains(&primary.len()) || !primary.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(ValueError::Malformed { field: Self::FIELD, expected: EXPECTED });
        }
        let mut canonical = primary.to_ascii_lowercase();
        for subtag in subtags {
            if !(1..=8).contains(&subtag.len()) || !subtag.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(ValueError::Malformed { field: Self::FIELD, expected: EXPECTED });
            }
            canonical.push('-');
            canonical.push_str(&case_of(subtag));
        }
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The language's name in English and in the language itself, e.g.
    /// `("German", "Deutsch")` - for a model, which follows a name more
    /// surely than a tag. Found by the tag, or by what is left of it
    /// without its last subtags (`de-CH` is German); `None` for a language
    /// the table does not know, which is then named by its tag alone.
    pub fn names(&self) -> Option<(&'static str, &'static str)> {
        let mut tag = self.0.as_str();
        loop {
            if let Some((_, english, native)) = NAMES.iter().find(|(known, ..)| *known == tag) {
                return Some((english, native));
            }
            tag = &tag[..tag.rfind('-')?];
        }
    }
}

/// The names of common languages by their tags, in English and in the
/// language itself: those the web UI suggests, and some more.
const NAMES: &[(&str, &str, &str)] = &[
    ("ar", "Arabic", "العربية"),
    ("bg", "Bulgarian", "български"),
    ("ca", "Catalan", "català"),
    ("cs", "Czech", "čeština"),
    ("da", "Danish", "dansk"),
    ("de", "German", "Deutsch"),
    ("el", "Greek", "Ελληνικά"),
    ("en", "English", "English"),
    ("en-GB", "British English", "British English"),
    ("en-US", "American English", "American English"),
    ("es", "Spanish", "español"),
    ("et", "Estonian", "eesti"),
    ("fa", "Persian", "فارسی"),
    ("fi", "Finnish", "suomi"),
    ("fr", "French", "français"),
    ("he", "Hebrew", "עברית"),
    ("hi", "Hindi", "हिन्दी"),
    ("hr", "Croatian", "hrvatski"),
    ("hu", "Hungarian", "magyar"),
    ("id", "Indonesian", "Bahasa Indonesia"),
    ("it", "Italian", "italiano"),
    ("ja", "Japanese", "日本語"),
    ("ko", "Korean", "한국어"),
    ("lt", "Lithuanian", "lietuvių"),
    ("lv", "Latvian", "latviešu"),
    ("nb", "Norwegian Bokmål", "norsk bokmål"),
    ("nl", "Dutch", "Nederlands"),
    ("nn", "Norwegian Nynorsk", "norsk nynorsk"),
    ("no", "Norwegian", "norsk"),
    ("pl", "Polish", "polski"),
    ("pt", "Portuguese", "português"),
    ("pt-BR", "Brazilian Portuguese", "português do Brasil"),
    ("ro", "Romanian", "română"),
    ("ru", "Russian", "русский"),
    ("sk", "Slovak", "slovenčina"),
    ("sl", "Slovenian", "slovenščina"),
    ("sr", "Serbian", "српски"),
    ("sv", "Swedish", "svenska"),
    ("th", "Thai", "ไทย"),
    ("tr", "Turkish", "Türkçe"),
    ("uk", "Ukrainian", "українська"),
    ("vi", "Vietnamese", "Tiếng Việt"),
    ("zh", "Chinese", "中文"),
    ("zh-Hans", "Simplified Chinese", "简体中文"),
    ("zh-Hant", "Traditional Chinese", "繁體中文"),
];

/// A subtag after the primary language in its usual case: a script (four
/// letters) capitalised, a region (two letters) in upper case, anything
/// else in lower case.
fn case_of(subtag: &str) -> String {
    let letters = subtag.chars().all(|c| c.is_ascii_alphabetic());
    match subtag.len() {
        4 if letters => {
            let (first, rest) = subtag.split_at(1);
            first.to_ascii_uppercase() + &rest.to_ascii_lowercase()
        }
        2 if letters => subtag.to_ascii_uppercase(),
        _ => subtag.to_ascii_lowercase(),
    }
}

impl_value!([] Language, "language");

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(text: &str) -> String {
        Language::new(text).expect("a language").to_string()
    }

    #[test]
    fn a_language_is_kept_in_the_usual_case() {
        assert_eq!(tag(" de "), "de");
        assert_eq!(tag("DE-ch"), "de-CH");
        assert_eq!(tag("zh-hant-tw"), "zh-Hant-TW");
        assert_eq!(tag("en_GB"), "en-GB", "an underscore is taken as a hyphen");
        assert_eq!(tag("es-419"), "es-419", "a numeric region stays as it is");
        assert_eq!(tag("de-CH-1996"), "de-CH-1996");
    }

    #[test]
    fn only_a_tag_is_a_language() {
        for text in ["", "d", "deut", "de-", "de--CH", "de CH", "dé", "1de", "de-toolongsubtag"] {
            assert_eq!(Language::new(text), Err(ValueError::Malformed { field: "language", expected: EXPECTED }), "{text:?}");
        }
        assert_eq!(Language::new(format!("de{}", "-a".repeat(MAX_LANGUAGE_CHARS))), Err(ValueError::TooLong { field: "language", max_chars: MAX_LANGUAGE_CHARS }));
        assert_eq!(Language::new("Deutsch").unwrap_err().to_string(), "the language must be a tag such as \"de\" or \"en-GB\"", "naming the field");
        assert_eq!(Language::optional(Some("")), Ok(None), "empty is none");
        assert!(Language::optional(Some(" ")).is_err(), "whitespace is no language");
    }

    #[test]
    fn a_language_is_named_by_its_tag_or_what_is_left_of_it() {
        let names = |text: &str| Language::new(text).unwrap().names();
        assert_eq!(names("de"), Some(("German", "Deutsch")));
        assert_eq!(names("de-CH-1996"), Some(("German", "Deutsch")), "without its region and variant");
        assert_eq!(names("zh-Hant-TW"), Some(("Traditional Chinese", "繁體中文")), "as far as the table knows it");
        assert_eq!(names("pt-BR"), Some(("Brazilian Portuguese", "português do Brasil")));
        assert_eq!(names("tlh"), None, "a language the table does not know");
        assert!(NAMES.iter().all(|(tag, ..)| Language::new(tag).is_ok_and(|language| language.as_str() == *tag)), "every tag of the table as a language keeps it");
    }

    #[test]
    fn a_language_is_written_as_its_tag_and_checked_when_read() {
        let language = Language::new("en-gb").unwrap();
        assert_eq!(serde_json::to_string(&language).unwrap(), "\"en-GB\"");
        assert_eq!(serde_json::from_str::<Language>("\"pt_br\"").unwrap(), Language::new("pt-BR").unwrap());
        assert!(serde_json::from_str::<Language>("\"Deutsch\"").is_err());
    }
}
