//! Where a remote system is reached and how one authenticates there - the `endpoint` that
//! `ollama_client`, `mcp_tool`, `a2a_agent` and `decision_policy` are given in their `params` and a
//! storage of workspaces in its configuration (issues 179, 188, 251) - and how it is shown in
//! messages: without what may carry a credential.
//!
//! What is read as an endpoint has a URL a request can be sent to: one that is no `http` or
//! `https` URL is refused as it is read (issue 515), before anything is built of it and before a
//! request writes it, and without being named, since it may carry a credential.

use crate::secret::{SecretManager, SecretResult};
use crate::security::SecurityContext;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::cmp::Reverse;
use std::time::Duration;
use url::Url;

/// Where and how a component reaches its remote side, written in its
/// `params` as `endpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteEndpoint {
    /// An `http` or `https` URL, refused as it is read unless it is one.
    #[serde(deserialize_with = "http_url")]
    pub url: String,
    #[serde(default)]
    pub auth: Option<RemoteAuthConfig>,
    /// How long the component waits for its remote side; what it bounds,
    /// and how long without it, each component says.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// How long what the component reads of its remote side - an MCP
    /// server's tools, an A2A agent's card - is kept before it is read
    /// again (issue 126); what it reads, and how long without it, each
    /// component says.
    #[serde(default)]
    pub refresh_secs: Option<u64>,
}

impl RemoteEndpoint {
    /// `url`, without authentication, a timeout or a time to keep what is
    /// read.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), auth: None, timeout_secs: None, refresh_secs: None }
    }

    /// The URL, for messages: without credentials.
    pub fn shown(&self) -> String {
        redact_endpoint(&self.url)
    }

    /// The URL of `segments` below its own, each encoded as one segment, its query kept - without
    /// any, its own without a trailing `/`. `None` for a URL that is none, or one with no path to
    /// go below, such as a `mailto:` - which only one not read as parameters can be.
    pub fn url_of(&self, segments: &[&str]) -> Option<Url> {
        let mut url = Url::parse(&self.url).ok()?;
        url.path_segments_mut().ok()?.pop_if_empty().extend(segments);
        Some(url)
    }

    /// `text` - a message of a library that writes the URL into its own -
    /// with the URL shown instead: as written, as a parser normalizes it,
    /// and as a client sends it, its credentials moved into a header.
    pub fn redact_in(&self, text: &str) -> String {
        let mut forms = vec![self.url.clone()];
        if let Ok(mut parsed) = Url::parse(&self.url) {
            forms.push(parsed.to_string());
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            forms.push(parsed.to_string());
        }
        // The longest first, so that no form is left half replaced.
        forms.sort_by_key(|form| Reverse(form.len()));
        let shown = self.shown();
        forms.iter().fold(text.to_string(), |text, form| text.replace(form.as_str(), &shown))
    }

    /// The timeout, `default` unless it is written.
    pub fn timeout_or(&self, default: Duration) -> Duration {
        self.timeout_secs.map_or(default, Duration::from_secs)
    }

    /// How long what is read of the remote side is kept, `default` unless
    /// it is written.
    pub fn refresh_or(&self, default: Duration) -> Duration {
        self.refresh_secs.map_or(default, Duration::from_secs)
    }

    /// The bearer token `auth` names, resolved in `context`; `None` without
    /// authentication. A secret is resolved on every use, never kept.
    ///
    /// # Errors
    /// Whatever resolving the secret reports.
    pub async fn bearer_token(
        &self,
        secrets: &SecretManager,
        context: &SecurityContext,
    ) -> SecretResult<Option<String>> {
        match &self.auth {
            None => Ok(None),
            Some(RemoteAuthConfig::Bearer { secret }) => {
                Ok(Some(secrets.resolve(context, secret).await?))
            }
        }
    }
}

/// The `url` of a `RemoteEndpoint` read by `deserializer`.
///
/// # Errors
/// The deserializer's error for a URL that is no `http` or `https` one, without naming it.
fn http_url<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let url = String::deserialize(deserializer)?;
    match Url::parse(&url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => Ok(url),
        _ => Err(D::Error::custom("it is no http or https URL")),
    }
}

/// How a component authenticates at its remote side, written in its
/// `endpoint` as `auth`. A secret is named by its key, resolved through the
/// `SecretManager` in the context of whoever acts, on every use - never
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteAuthConfig {
    /// A bearer token: the secret `secret`.
    Bearer { secret: String },
}

/// `url` without user name, password, query and fragment, for use in
/// messages: any of them may carry a credential (issue 188). Where it
/// cannot be parsed, nothing of it is shown.
pub fn redact_endpoint(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => "(an invalid URL)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_endpoint_is_shown_without_what_may_carry_a_credential() {
        assert_eq!(
            redact_endpoint("https://user:secret@agent.example/a2a/?key=secret#secret"),
            "https://agent.example/a2a"
        );
        assert_eq!(redact_endpoint("http://127.0.0.1:11434"), "http://127.0.0.1:11434");
        assert!(!redact_endpoint("not a url with secret").contains("secret"));
    }

    #[test]
    fn an_endpoint_whose_url_is_no_http_one_is_refused_without_naming_it() {
        let read = |url: &str| serde_json::from_value::<RemoteEndpoint>(json!({ "url": url }));
        for url in ["http://127.0.0.1:11434", "https://user:secret@agent.example/a2a?key=secret"] {
            assert_eq!(read(url).unwrap().url, url, "as written");
        }
        for url in ["", "secret", "agent.example", "file:///secret", "mailto:secret@example"] {
            let refused = read(url).unwrap_err().to_string();
            assert!(
                refused.contains("no http or https URL") && !refused.contains("secret"),
                "{url:?}: {refused}"
            );
        }
    }

    #[test]
    fn a_url_below_the_endpoint_keeps_its_query_and_encodes_each_segment() {
        let below = |url: &str, segments: &[&str]| {
            RemoteEndpoint::new(url).url_of(segments).map(|url| url.to_string())
        };
        for url in ["http://127.0.0.1:11434", "http://127.0.0.1:11434/"] {
            assert_eq!(
                below(url, &["api", "chat"]).as_deref(),
                Some("http://127.0.0.1:11434/api/chat"),
                "{url}"
            );
        }
        assert_eq!(
            below("https://user:secret@agent.example/a2a/?key=secret", &["v1", "a/b"]).as_deref(),
            Some("https://user:secret@agent.example/a2a/v1/a%2Fb?key=secret"),
            "below its path, its query kept, a segment as one"
        );
        assert_eq!(
            below("https://agent.example/card.json", &[]).as_deref(),
            Some("https://agent.example/card.json")
        );
        assert_eq!(below("agent.example", &["api"]), None, "no URL");
        assert_eq!(below("mailto:agent@example", &["api"]), None, "none below which a path lies");
    }

    #[test]
    fn the_url_a_library_writes_into_its_message_is_shown_instead() {
        let endpoint = RemoteEndpoint::new("http://user:secret@mcp.example?key=secret");
        let message = "error sending request for url (http://user:secret@mcp.example/?key=secret)";
        assert_eq!(
            endpoint.redact_in(message),
            "error sending request for url (http://mcp.example)"
        );
        let sent = "error sending request for url (http://mcp.example/?key=secret)";
        assert_eq!(
            endpoint.redact_in(sent),
            "error sending request for url (http://mcp.example)",
            "the credentials in a header"
        );
    }
}
