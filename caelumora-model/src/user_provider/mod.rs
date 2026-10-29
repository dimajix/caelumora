//! Sources of user identities and verification of their credentials (issue 25).
//!
//! A `UserProvider` is one source of users — a file, a database, a directory —
//! and a `UserManager` holds an ordered chain of them, mirroring
//! `SecretProvider` / `SecretManager`. Unlike the secret chain, a provider that
//! fails does *not* fall through to the next one: for authentication, "the
//! user file is broken" must never turn into "ask someone else" or into
//! "no such user".

pub mod accept_any;
pub mod file;
mod manager;

pub use manager::UserManager;

use crate::user::{User, UserId};
use async_trait::async_trait;
use std::fmt;
use std::fmt::Debug;

/// A credential presented for authentication.
///
/// Borrowed, so that callers do not scatter copies of secret material, and
/// with a `Debug` impl that never prints it. More kinds (bearer tokens, API
/// keys) are expected; a provider that cannot verify a kind declines it.
#[non_exhaustive]
pub enum Credential<'a> {
    Password(&'a str),
    /// No secret: a frontend that runs in the runtime's own process - the TUI - vouches for whom
    /// it acts for. It holds the stores itself, so a secret would keep out nobody it does not
    /// already let in. Every provider takes it for the users it knows, also one that verifies a
    /// secret otherwise, so that the chain answers it for a user where it answers a password. It
    /// never reaches the REST server's login, which takes a password.
    Trusted,
}

impl Credential<'_> {
    /// A short name for the credential kind, for logs and errors.
    pub fn kind(&self) -> &'static str {
        match self {
            Credential::Password(_) => "password",
            Credential::Trusted => "trusted",
        }
    }
}

impl Debug for Credential<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential::{}(<redacted>)", self.kind())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The credential was rejected, or no provider knows the user. The two are
    /// deliberately not distinguished towards the caller, so that a login form
    /// cannot be used to find out which usernames exist.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// A provider could not answer, e.g. an unreadable or malformed user file
    /// or an unreachable directory. Authentication is denied, never passed on.
    #[error("user provider failed: {0}")]
    Backend(String),
}

pub type AuthResult<T> = Result<T, AuthError>;

/// A single source of users, consulted in order by a `UserManager`.
///
/// A provider says who a user is by its source - id, name, roles - and
/// writes nothing: the `UserManager` keeps the records (issue 306).
///
/// Looking a user up and verifying a credential are separate operations, and
/// verification never hands out the stored credential: a file backend compares
/// a hash, a directory verifies by binding as the user and has nothing to hand
/// out at all.
///
/// **Ids.** How a provider assigns `UserId`s is up to the provider — random
/// and persisted, derived from the username, or taken from the backend's own
/// immutable key. The contract is only that it returns the same id for the
/// same user for as long as it knows that user, including across restarts:
/// the id ends up in ownership references throughout the stores.
///
/// For all three methods, `Ok(None)` means "not mine, ask the next provider";
/// an `Err` stops the chain.
#[async_trait]
pub trait UserProvider: Debug + Send + Sync {
    /// Looks up a user by login name, without verifying anything.
    async fn find_user(&self, username: &str) -> AuthResult<Option<User>>;

    /// Looks up a user by id, e.g. to check that the subject of a session
    /// still exists and is enabled.
    async fn get_user(&self, id: UserId) -> AuthResult<Option<User>>;

    /// Verifies `credential` for `username` and returns the authenticated
    /// user. Returns `Ok(None)` if the user is unknown to this provider or it
    /// cannot verify this kind of credential, and
    /// `Err(AuthError::InvalidCredentials)` if the user is known here but the
    /// credential is wrong — which ends the chain, so that a second provider
    /// knowing the same name cannot be used to bypass the first.
    /// `Credential::Trusted` it takes for every user it knows.
    async fn authenticate(
        &self,
        username: &str,
        credential: &Credential<'_>,
    ) -> AuthResult<Option<User>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_debug_never_prints_the_secret() {
        let rendered = format!("{:?}", Credential::Password("hunter2"));
        assert!(!rendered.contains("hunter2"), "got {}", rendered);
    }
}
