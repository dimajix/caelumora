//! Where a request is sent (issue 310): `Origin`, the scheme, host and port of an `http` or `https`
//! URL - a variant of the texts.

use super::ValueError;
use url::Url;

/// What an origin looks like, in the error for one that does not.
const EXPECTED: &str = "an http or https origin, written scheme://host or scheme://host:port";

/// The origin of a URL, where a request to it goes: its scheme, `http` or `https`, its host and
/// its port, written as the URL standard writes an origin - `https://api.example`, in lower case,
/// with no port that is the scheme's own, and nothing else -, so that it is written one way only
/// and compared as written. Written down as a string, and checked when read.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin(String);

impl Origin {
    /// `text` as an origin.
    ///
    /// # Errors
    /// `ValueError` if it is empty, no `http` or `https` URL, or anything but an origin as the
    /// standard writes it.
    pub fn new(text: impl AsRef<str>) -> Result<Self, ValueError> {
        let text = text.as_ref();
        if text.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        match Self::of(text) {
            Some(origin) if origin.0 == text => Ok(origin),
            _ => Err(ValueError::Malformed { field: Self::FIELD, expected: EXPECTED }),
        }
    }

    /// The origin of `url`, where a request to it is sent; `None` for a text that is no `http` or
    /// `https` URL.
    pub fn of(url: &str) -> Option<Self> {
        let parsed = Url::parse(url).ok()?;
        matches!(parsed.scheme(), "http" | "https")
            .then(|| Self(parsed.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_value!([] Origin, "origin");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_is_written_as_the_standard_writes_it_and_nothing_else() {
        for text in ["https://api.tavily.com", "http://127.0.0.1:8000", "https://mcp.example:8443"]
        {
            assert_eq!(Origin::new(text).unwrap().as_str(), text);
        }
        assert!(matches!(Origin::new(""), Err(ValueError::Empty { .. })));
        for text in [
            "https://api.tavily.com/",
            "https://api.tavily.com/search",
            "https://API.tavily.com",
            "https://api.tavily.com:443",
            "https://user@api.tavily.com",
            "ftp://files.example",
            "api.tavily.com",
            "*",
        ] {
            assert!(matches!(Origin::new(text), Err(ValueError::Malformed { .. })), "{text}");
        }
    }

    #[test]
    fn the_origin_of_a_url_is_where_a_request_to_it_goes() {
        let origin = |url: &str| Origin::of(url).map(|o| o.to_string());
        assert_eq!(
            origin("https://user:secret@API.example:443/mcp?key=secret").as_deref(),
            Some("https://api.example"),
            "without credentials, path, query and the scheme's own port"
        );
        assert_eq!(origin("http://127.0.0.1:8000/v1").as_deref(), Some("http://127.0.0.1:8000"));
        assert_eq!(origin("mailto:someone@example"), None);
        assert_eq!(origin("no url"), None);
    }
}
