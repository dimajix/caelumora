//! What is read of a secret, and how reading one fails (issue 516): the secrets bound to one name
//! (`Secrets`), which a component that talks to a remote system resolves its credential by. Where
//! they come from, and who may read them, is `caelumora-security`'s (`SecretManager`, issue 517).

use crate::security::Refused;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found: {0}")]
    NotFound(String),
    #[error("access denied for secret: {key}")]
    PermissionDenied {
        key: String,
        #[source]
        refused: Refused,
    },
    #[error("backend: {0}")]
    Backend(String),
}

pub type SecretResult<T> = Result<T, SecretError>;

/// The secrets that may be read in one name, bound to it by the kernel (issue 516): what a
/// component that talks to a remote system resolves its credential by, without naming the name or
/// holding the `SecretManager`. Each read is asked of the policy (`ReadSecret`) in the name it is
/// bound to.
#[async_trait]
pub trait Secrets: Send + Sync {
    /// The secret `key`.
    ///
    /// # Errors
    /// As `SecretManager::resolve`.
    async fn resolve(&self, key: &str) -> SecretResult<String>;
}
