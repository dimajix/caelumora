//! What every component that talks to a remote system shares: how it
//! authenticates there, how its endpoint is shown, and how its errors are
//! described. `mcp_tool` and `a2a_agent` configure it alike (issue 179);
//! `ollama_client` shows and describes alike (issue 188).

use caelumora_model::secret::{SecretManager, SecretResult};
use caelumora_model::security::SecurityContext;
use serde::Deserialize;

/// How a component authenticates at its remote side, written in its
/// `params` as `auth`. A secret is named by its key, resolved through the
/// `SecretManager` in the context of whoever acts, on every use - never
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RemoteAuthConfig {
    /// A bearer token: the secret `secret`.
    Bearer { secret: String },
}

impl RemoteAuthConfig {
    /// The bearer token `auth` names, resolved in `context`; `None` without
    /// authentication.
    ///
    /// # Errors
    /// Whatever resolving the secret reports.
    pub(crate) async fn bearer_token(auth: Option<&RemoteAuthConfig>, secrets: &SecretManager, context: &SecurityContext) -> SecretResult<Option<String>> {
        match auth {
            None => Ok(None),
            Some(RemoteAuthConfig::Bearer { secret }) => Ok(Some(secrets.resolve(context, secret).await?)),
        }
    }
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
