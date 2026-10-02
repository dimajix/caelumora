//! What every component that talks to a remote system shares: how it
//! authenticates there. `mcp_tool` and `a2a_agent` configure it alike
//! (issue 179).

use caelumora_model::secret::{SecretManager, SecretResult};
use caelumora_model::security::SecurityContext;
use serde::Deserialize;

/// How a component authenticates at its remote side, written in its
/// `params` as `auth`. A secret is named by its key, resolved through the
/// `SecretManager` in the context of whoever acts, on every use - never
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteAuthConfig {
    /// A bearer token: the secret `secret`.
    Bearer { secret: String },
}

impl RemoteAuthConfig {
    /// The bearer token `auth` names, resolved in `context`; `None` without
    /// authentication.
    ///
    /// # Errors
    /// Whatever resolving the secret reports.
    pub async fn bearer_token(auth: Option<&RemoteAuthConfig>, secrets: &SecretManager, context: &SecurityContext) -> SecretResult<Option<String>> {
        match auth {
            None => Ok(None),
            Some(RemoteAuthConfig::Bearer { secret }) => Ok(Some(secrets.resolve(context, secret).await?)),
        }
    }
}
