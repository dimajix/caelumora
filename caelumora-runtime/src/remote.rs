//! What every component that talks to a remote system shares: where it
//! reaches it and how it authenticates there - its `RemoteEndpoint`,
//! written alike by `ollama_client`, `mcp_tool` and `a2a_agent` (issues 179,
//! 188) - how its endpoint is shown, and how its errors are described.

use caelumora_model::secret::{SecretManager, SecretResult};
use caelumora_model::security::SecurityContext;
use serde::Deserialize;
use std::time::Duration;

/// Where and how a component reaches its remote side, written in its
/// `params` as `endpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteEndpoint {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) auth: Option<RemoteAuthConfig>,
    /// How long the component waits for its remote side; what it bounds,
    /// and how long without it, each component says.
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
}

impl RemoteEndpoint {
    /// `url`, without authentication or a timeout.
    pub(crate) fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), auth: None, timeout_secs: None }
    }

    /// The URL, for messages: without credentials.
    pub(crate) fn shown(&self) -> String {
        redact_endpoint(&self.url)
    }

    /// `text` - a message of a library that writes the URL into its own -
    /// with the URL shown instead: as written, as a parser normalizes it,
    /// and as a client sends it, its credentials moved into a header.
    pub(crate) fn redact_in(&self, text: &str) -> String {
        let mut forms = vec![self.url.clone()];
        if let Ok(mut parsed) = reqwest::Url::parse(&self.url) {
            forms.push(parsed.to_string());
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            forms.push(parsed.to_string());
        }
        // The longest first, so that no form is left half replaced.
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        let shown = self.shown();
        forms.iter().fold(text.to_string(), |text, form| text.replace(form.as_str(), &shown))
    }

    /// The timeout, `default` unless it is written.
    pub(crate) fn timeout_or(&self, default: Duration) -> Duration {
        self.timeout_secs.map_or(default, Duration::from_secs)
    }

    /// The bearer token `auth` names, resolved in `context`; `None` without
    /// authentication. A secret is resolved on every use, never kept.
    ///
    /// # Errors
    /// Whatever resolving the secret reports.
    pub(crate) async fn bearer_token(&self, secrets: &SecretManager, context: &SecurityContext) -> SecretResult<Option<String>> {
        match &self.auth {
            None => Ok(None),
            Some(RemoteAuthConfig::Bearer { secret }) => Ok(Some(secrets.resolve(context, secret).await?)),
        }
    }
}

/// How a component authenticates at its remote side, written in its
/// `endpoint` as `auth`. A secret is named by its key, resolved through the
/// `SecretManager` in the context of whoever acts, on every use - never
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RemoteAuthConfig {
    /// A bearer token: the secret `secret`.
    Bearer { secret: String },
}

/// `url` without user name, password, query and fragment, for use in
/// messages: any of them may carry a credential (issue 188). Where it
/// cannot be parsed, nothing of it is shown.
pub(crate) fn redact_endpoint(url: &str) -> String {
    match reqwest::Url::parse(url) {
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

/// The full chain of the causes of `error`, a failed request, without the
/// URL that `reqwest` writes into its messages.
pub(crate) fn describe_request_error(error: reqwest::Error) -> String {
    describe_chain(&error.without_url())
}

/// The full chain of an error's causes as one line, e.g. "error sending
/// request: connection refused", for errors whose own `Display` stops at
/// the outermost layer.
pub(crate) fn describe_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        let text = e.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        current = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_is_shown_without_what_may_carry_a_credential() {
        assert_eq!(redact_endpoint("https://user:secret@agent.example/a2a/?key=secret#secret"), "https://agent.example/a2a");
        assert_eq!(redact_endpoint("http://127.0.0.1:11434"), "http://127.0.0.1:11434");
        assert!(!redact_endpoint("not a url with secret").contains("secret"));
    }

    #[test]
    fn the_url_a_library_writes_into_its_message_is_shown_instead() {
        let endpoint = RemoteEndpoint::new("http://user:secret@mcp.example?key=secret");
        let message = "error sending request for url (http://user:secret@mcp.example/?key=secret)";
        assert_eq!(endpoint.redact_in(message), "error sending request for url (http://mcp.example)");
        let sent = "error sending request for url (http://mcp.example/?key=secret)";
        assert_eq!(endpoint.redact_in(sent), "error sending request for url (http://mcp.example)", "the credentials in a header");
    }
}
