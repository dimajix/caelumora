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

/// `url` without user name and password, for use in messages.
pub(crate) fn redact_endpoint(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => url.to_string(),
    }
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
