//! What is read of a secret, and how reading one fails (issue 516): the secrets bound to one name
//! (`Secrets`), which a component that talks to a remote system resolves its credential by, by
//! what names it (`SecretRef`, issue 310). Where they come from, and who may read them, is
//! `caelumora-security`'s (`SecretManager`, issue 517).

use crate::security::Refused;
use crate::value::origin::Origin;
use crate::value::secret_ref::SecretRef;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found: {0}")]
    NotFound(SecretRef),
    #[error("access denied for secret: {secret}")]
    PermissionDenied {
        secret: SecretRef,
        #[source]
        refused: Refused,
    },
    /// A reference to a vault that is not configured.
    #[error("the vault of the secret '{0}' is not configured")]
    UnknownVault(SecretRef),
    /// A secret its vault does not let be sent where it was to be sent: to the origin of the URL,
    /// or - `None` - to what is no `http` or `https` URL.
    #[error(
        "the secret '{secret}' may not be sent to {}",
        .origin.as_ref().map_or("what is no http or https URL", Origin::as_str)
    )]
    NotSentTo { secret: SecretRef, origin: Option<Origin> },
    #[error("backend: {0}")]
    Backend(String),
}

pub type SecretResult<T> = Result<T, SecretError>;

/// The secrets that may be read in one name, bound to it by the kernel (issue 516): what a
/// component that talks to a remote system resolves its credential by, without naming the name or
/// holding the `SecretManager`. Each read is asked of the policy - whether the vault may be used -
/// in the name it is bound to.
#[async_trait]
pub trait Secrets: Send + Sync {
    /// The secret `secret`, to be sent to `sent_to`, the URL of the request it goes with: a
    /// component's credential always goes somewhere (issue 310).
    ///
    /// # Errors
    /// As `SecretManager::resolve`.
    async fn resolve(&self, secret: &SecretRef, sent_to: &str) -> SecretResult<String>;

    /// Whether `secret` may be used and sent to `sent_to`, as resolving it asks of its vault, but
    /// without reading it: for a configuration that names it (issue 310).
    ///
    /// # Errors
    /// As `SecretManager::require_usable`.
    async fn require_usable(&self, secret: &SecretRef, sent_to: &str) -> SecretResult<()>;
}
