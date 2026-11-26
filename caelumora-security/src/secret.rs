//! The secrets of a process (issues 310, 516): its named vaults (`Vault`), each a source of secrets
//! (`SecretProvider`) with what the configuration gives it, and the `SecretManager` that asks the
//! policy before a vault is read, in the name it is asked in. What is read, what names it and how
//! reading fails are the domain's (`caelumora_domain::secret`).

use crate::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use async_trait::async_trait;
use caelumora_domain::config::VaultConfig;
use caelumora_domain::secret::{SecretError, SecretResult, Secrets};
use caelumora_domain::value::identifier::Identifier;
use caelumora_domain::value::secret_ref::SecretRef;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::sync::Arc;

/// A single source of secrets, the backend of a `Vault`.
///
/// `Ok(None)` means the key is not there, and the vault's fallback is asked; `Err` means this
/// provider itself malfunctioned (e.g. a corrupt secrets file), which fails the lookup.
#[async_trait]
pub trait SecretProvider: Debug + Sync + Send {
    async fn get_secret(&self, key: &str) -> SecretResult<Option<String>>;
}

/// A vault of secrets (issue 310): a backend, and beside it what the configuration gives it - the
/// vault asked for a key it does not have. The backend knows nothing of it.
#[derive(Debug)]
pub struct Vault {
    provider: Arc<dyn SecretProvider>,
    fallback: Option<Identifier>,
}

impl Vault {
    /// The vault of `provider`, as `config` configures it.
    pub fn new(provider: Arc<dyn SecretProvider>, config: &VaultConfig) -> Self {
        Self { provider, fallback: config.fallback.clone() }
    }
}

#[derive(Debug)]
pub struct SecretManager {
    vaults: BTreeMap<Identifier, Vault>,
    access_policy: Arc<dyn AccessPolicy>,
}

impl SecretManager {
    /// The manager of `vaults`, by their names, asking `access_policy`.
    ///
    /// # Errors
    /// `SecretError::InvalidVaults` for a vault named `user` - the principal's, which no
    /// configuration names (`SecretRef::USER_VAULT`) -, a fallback that is not configured, and
    /// fallbacks that lead back to where they began.
    pub fn new(
        vaults: BTreeMap<Identifier, Vault>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> SecretResult<Self> {
        if vaults.contains_key(SecretRef::USER_VAULT) {
            return Err(SecretError::InvalidVaults(format!(
                "'{}' is the vault of whoever acts, which no configuration names",
                SecretRef::USER_VAULT
            )));
        }
        for name in vaults.keys() {
            let mut seen = BTreeSet::from([name]);
            let mut next = vaults.get(name).and_then(|vault| vault.fallback.as_ref());
            while let Some(fallback) = next {
                if !vaults.contains_key(fallback) {
                    return Err(SecretError::InvalidVaults(format!(
                        "the vault '{name}' falls back to '{fallback}', which is not configured"
                    )));
                }
                if !seen.insert(fallback) {
                    return Err(SecretError::InvalidVaults(format!(
                        "the fallbacks of the vault '{name}' lead back to '{fallback}'"
                    )));
                }
                next = vaults.get(fallback).and_then(|vault| vault.fallback.as_ref());
            }
        }
        Ok(Self { vaults, access_policy })
    }

    /// The secrets that may be read in the name of `context` (issue 516), each read asked of the
    /// policy then.
    pub fn for_context(self: &Arc<Self>, context: SecurityContext) -> SecretsFor {
        SecretsFor { manager: Arc::clone(self), context }
    }

    /// The secret `secret`, read in the name of `context`: of its vault, or - where the vault does
    /// not have the key - of its fallback, in turn.
    ///
    /// # Errors
    /// `PermissionDenied` if the policy refuses it, `UnknownVault` for a vault that is not
    /// configured, `NotFound` if no vault of the way has the key, and the error of a backend that
    /// fails - its fallback is not asked then, so that another credential is never sent in its
    /// stead.
    pub async fn resolve(
        &self,
        context: &SecurityContext,
        secret: &SecretRef,
    ) -> SecretResult<String> {
        self.access_policy
            .require(context, ActionType::ReadSecret(secret.as_str()), |refused| {
                SecretError::PermissionDenied { secret: secret.clone(), refused }
            })
            .await?;

        let mut name = secret.vault();
        loop {
            // Only the vault named can be missing: every fallback is configured (`new`).
            let vault =
                self.vaults.get(name).ok_or_else(|| SecretError::UnknownVault(secret.clone()))?;
            if let Some(value) = vault.provider.get_secret(secret.key()).await? {
                return Ok(value);
            }
            // The fallbacks lead nowhere back (`new`), so the way ends.
            match &vault.fallback {
                Some(fallback) => name = fallback.as_str(),
                None => return Err(SecretError::NotFound(secret.clone())),
            }
        }
    }
}

/// The managers the tests of other crates read secrets by, past its own API.
#[cfg(any(test, feature = "test-support"))]
impl SecretManager {
    /// The vault a test keeps its secrets in (`of_test_vault`): a test names them `test/KEY`.
    const TEST_VAULT: &'static str = "test";

    /// A manager of no vaults, asking `access_policy`: every secret is unknown - for a test that
    /// reads none.
    pub fn without_vaults(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self { vaults: BTreeMap::new(), access_policy }
    }

    /// A manager of the one vault `test`, of `provider`, asking `access_policy` - for a test of
    /// what resolves a secret, which names it `test/KEY`.
    #[expect(clippy::expect_used, reason = "test code: the name of the vault is a literal")]
    pub fn of_test_vault(
        provider: Arc<dyn SecretProvider>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        let name = Identifier::new(Self::TEST_VAULT).expect("`test` is an identifier");
        let vault = Vault { provider, fallback: None };
        Self { vaults: BTreeMap::from([(name, vault)]), access_policy }
    }
}

/// The secrets of a `SecretManager` that may be read in one name (`SecretManager::for_context`).
#[derive(Debug, Clone)]
pub struct SecretsFor {
    manager: Arc<SecretManager>,
    context: SecurityContext,
}

#[async_trait]
impl Secrets for SecretsFor {
    async fn resolve(&self, secret: &SecretRef) -> SecretResult<String> {
        self.manager.resolve(&self.context, secret).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AllowAllPolicy;
    use caelumora_domain::config::BackendConfig;
    use serde_json::Value;
    use std::collections::HashMap;

    #[derive(Debug)]
    struct MapSecretProvider {
        entries: HashMap<String, String>,
    }

    #[async_trait]
    impl SecretProvider for MapSecretProvider {
        async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
            Ok(self.entries.get(key).cloned())
        }
    }

    #[derive(Debug)]
    struct FailingSecretProvider;

    #[async_trait]
    impl SecretProvider for FailingSecretProvider {
        async fn get_secret(&self, _key: &str) -> SecretResult<Option<String>> {
            Err(SecretError::Backend("boom".to_string()))
        }
    }

    #[derive(Debug)]
    struct DenyAllPolicy;

    #[async_trait]
    impl AccessPolicy for DenyAllPolicy {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    /// A provider of `entries`.
    fn holding(entries: &[(&str, &str)]) -> Arc<dyn SecretProvider> {
        let entries = entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Arc::new(MapSecretProvider { entries })
    }

    /// The vault `name` of `provider`, falling back to `fallback` if given.
    fn vault(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        fallback: Option<&str>,
    ) -> (Identifier, Vault) {
        let fallback = fallback.map(|f| Identifier::new(f).unwrap());
        let config =
            VaultConfig { fallback, ..VaultConfig::new(BackendConfig::new("test", Value::Null)) };
        (Identifier::new(name).unwrap(), Vault::new(provider, &config))
    }

    fn manager(
        vaults: impl IntoIterator<Item = (Identifier, Vault)>,
    ) -> SecretResult<SecretManager> {
        SecretManager::new(vaults.into_iter().collect(), Arc::new(AllowAllPolicy::new()))
    }

    fn secret(text: &str) -> SecretRef {
        SecretRef::new(text).unwrap()
    }

    #[tokio::test]
    async fn a_secret_is_read_of_its_vault_alone() {
        let manager = manager([
            vault("team", holding(&[("KEY", "team's")]), None),
            vault("ops", holding(&[("KEY", "ops'")]), None),
        ])
        .unwrap();
        let context = SecurityContext::default();
        assert_eq!(manager.resolve(&context, &secret("ops/KEY")).await.unwrap(), "ops'");
        assert_eq!(manager.resolve(&context, &secret("team/KEY")).await.unwrap(), "team's");
        let unknown = manager.resolve(&context, &secret("dev/KEY")).await.unwrap_err();
        assert!(matches!(unknown, SecretError::UnknownVault(s) if s.vault() == "dev"));
        let user = manager.resolve(&context, &secret("user/KEY")).await.unwrap_err();
        assert!(matches!(user, SecretError::UnknownVault(_)), "found nowhere until issue 343");
    }

    #[tokio::test]
    async fn a_key_a_vault_does_not_have_is_asked_of_its_fallback_in_turn() {
        let manager = manager([
            vault("local", holding(&[("OWN", "local's")]), Some("team")),
            vault("team", holding(&[]), Some("env")),
            vault("env", holding(&[("OWN", "env's"), ("SHARED", "env's")]), None),
        ])
        .unwrap();
        let context = SecurityContext::default();
        assert_eq!(manager.resolve(&context, &secret("local/OWN")).await.unwrap(), "local's");
        assert_eq!(manager.resolve(&context, &secret("local/SHARED")).await.unwrap(), "env's");
        let missing = manager.resolve(&context, &secret("local/MISSING")).await.unwrap_err();
        assert!(matches!(missing, SecretError::NotFound(s) if s.as_str() == "local/MISSING"));
    }

    #[tokio::test]
    async fn a_failing_backend_fails_the_lookup_and_its_fallback_is_not_asked() {
        let manager = manager([
            vault("local", Arc::new(FailingSecretProvider), Some("env")),
            vault("env", holding(&[("KEY", "env's")]), None),
        ])
        .unwrap();
        let failed = manager.resolve(&SecurityContext::default(), &secret("local/KEY")).await;
        assert!(matches!(failed, Err(SecretError::Backend(_))), "{failed:?}");
    }

    #[test]
    fn vaults_that_cannot_be_used_together_are_refused() {
        let refused = |vaults: Vec<(Identifier, Vault)>, what: &str| {
            let message = manager(vaults).unwrap_err().to_string();
            assert!(message.contains(what), "{message}");
        };
        refused(vec![vault("user", holding(&[]), None)], "'user' is the vault of whoever acts");
        refused(vec![vault("local", holding(&[]), Some("env"))], "falls back to 'env'");
        refused(
            vec![
                vault("a", holding(&[]), Some("b")),
                vault("b", holding(&[]), Some("c")),
                vault("c", holding(&[]), Some("b")),
            ],
            "lead back to 'b'",
        );
        assert!(
            manager([vault("a", holding(&[]), Some("b")), vault("b", holding(&[]), None)]).is_ok()
        );
    }

    #[tokio::test]
    async fn access_policy_denial_blocks_lookup() {
        let vaults = [vault("local", holding(&[("KEY", "value")]), None)];
        let manager =
            SecretManager::new(vaults.into_iter().collect(), Arc::new(DenyAllPolicy)).unwrap();
        let err =
            manager.resolve(&SecurityContext::default(), &secret("local/KEY")).await.unwrap_err();
        assert!(matches!(err, SecretError::PermissionDenied { .. }));
    }
}
