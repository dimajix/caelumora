//! The secrets of a process (issues 310, 516): its named vaults (`Vault`), each a source of secrets
//! (`SecretProvider`) with what the configuration gives it, and the `SecretManager` that asks the
//! policy whether a vault may be used (`USE`) before it is read, in the name it is asked in. What
//! is read, what names it and how reading fails are the domain's (`caelumora_domain::secret`).

use crate::{AccessPolicy, ActionType, CustomVerb, Enforcement, Requirement, SecurityContext};
use async_trait::async_trait;
use caelumora_domain::config::{ConfigError, ConfigResult, VaultConfig};
use caelumora_domain::secret::{SecretError, SecretResult, Secrets};
use caelumora_domain::security::{Grant, SecurityAttributes};
use caelumora_domain::value::identifier::Identifier;
use caelumora_domain::value::origin::Origin;
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

/// Using a secret (issue 310), asked of the attributes of the vault it is read of
/// (`SecretRef::KIND`): granted by an entry of the vault's access list as reading is. Unlike
/// reading, what the system owns is not everyone's (`Requirement::Explicit`): a vault the
/// configuration grants nobody is the system's alone.
pub const USE: CustomVerb =
    CustomVerb { name: "Use", requires: Requirement::Explicit(Grant::Read) };

/// A vault of secrets (issue 310): a backend, and beside it what the configuration gives it - the
/// system's attributes with whom it grants the use of its secrets, the origins they may be sent to,
/// and the vault asked for a key it does not have. The backend knows nothing of them.
#[derive(Debug)]
pub struct Vault {
    provider: Arc<dyn SecretProvider>,
    attributes: SecurityAttributes,
    /// `None`: anywhere.
    origins: Option<Vec<Origin>>,
    fallback: Option<Identifier>,
}

impl Vault {
    /// The vault of `provider`, as `config` configures it.
    pub fn new(provider: Arc<dyn SecretProvider>, config: &VaultConfig) -> Self {
        let attributes =
            SecurityAttributes { acl: config.acl.clone(), ..SecurityAttributes::systems() };
        let (origins, fallback) = (config.origins.clone(), config.fallback.clone());
        Self { provider, attributes, origins, fallback }
    }

    /// Whether `secret` of it may be sent to `sent_to`, the URL of a request - anything that is
    /// sent nowhere (`None`) may, also of a vault whose origins are none.
    ///
    /// # Errors
    /// `SecretError::NotSentTo` if its origins do not hold the URL's, or it is no URL.
    fn require_sent_to(&self, secret: &SecretRef, sent_to: Option<&str>) -> SecretResult<()> {
        let (Some(origins), Some(url)) = (&self.origins, sent_to) else {
            return Ok(());
        };
        match Origin::of(url) {
            Some(origin) if origins.contains(&origin) => Ok(()),
            origin => Err(SecretError::NotSentTo { secret: secret.clone(), origin }),
        }
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
    /// `ConfigError::Invalid` - vaults that do not fit together - for a vault named `user` - the
    /// principal's, which no configuration names (`SecretRef::USER_VAULT`) -, a fallback that is
    /// not configured, and fallbacks that lead back to where they began.
    pub fn new(
        vaults: BTreeMap<Identifier, Vault>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> ConfigResult<Self> {
        if vaults.contains_key(SecretRef::USER_VAULT) {
            return Err(ConfigError::Invalid(format!(
                "'{}' is the vault of whoever acts, which no configuration names",
                SecretRef::USER_VAULT
            )));
        }
        for name in vaults.keys() {
            let mut seen = BTreeSet::from([name]);
            let mut next = vaults.get(name).and_then(|vault| vault.fallback.as_ref());
            while let Some(fallback) = next {
                if !vaults.contains_key(fallback) {
                    return Err(ConfigError::Invalid(format!(
                        "the vault '{name}' falls back to '{fallback}', which is not configured"
                    )));
                }
                if !seen.insert(fallback) {
                    return Err(ConfigError::Invalid(format!(
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

    /// The secret `secret`, read in the name of `context` to be sent to `sent_to` - the URL of a
    /// request, `None` for what the server uses itself -: of its vault, or - where the vault does
    /// not have the key - of its fallback, in turn. Each vault is asked only once the policy let
    /// `context` use it (`USE`) and its origins hold `sent_to`'s; a fallback refused is refused,
    /// though the vault before did not have the key.
    ///
    /// # Errors
    /// `PermissionDenied` if the policy refuses a vault of the way, `NotSentTo` if one of them may
    /// not be sent there, `UnknownVault` for a vault that is not configured, `NotFound` if no vault
    /// of the way has the key, and the error of a backend that fails - its fallback is not asked
    /// then, so that another credential is never sent in its stead.
    pub async fn resolve(
        &self,
        context: &SecurityContext,
        secret: &SecretRef,
        sent_to: Option<&str>,
    ) -> SecretResult<String> {
        let mut name = secret.vault();
        loop {
            // Only the vault named can be missing: every fallback is configured (`new`).
            let vault =
                self.vaults.get(name).ok_or_else(|| SecretError::UnknownVault(secret.clone()))?;
            let using = ActionType::custom(SecretRef::KIND, &USE, &vault.attributes);
            self.access_policy
                .require(context, using, |refused| SecretError::PermissionDenied {
                    secret: secret.clone(),
                    refused,
                })
                .await?;
            vault.require_sent_to(secret, sent_to)?;
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
    /// The vault a test keeps its secrets in, granted to everyone: a test names them `test/KEY`.
    const TEST_VAULT: &'static str = "test";

    /// The vault the kernel's tests keep from everybody but the system (`of_test_vaults`).
    const WITHHELD_VAULT: &'static str = "withheld";

    /// The vault of the kernel's tests whose secrets go to `BOUND_ORIGIN` alone (`of_test_vaults`).
    const BOUND_VAULT: &'static str = "bound";

    /// Where the secrets of `BOUND_VAULT` may be sent.
    const BOUND_ORIGIN: &'static str = "https://bound.example";

    /// A manager of no vaults, asking `access_policy`: every secret is unknown - for a test that
    /// reads none.
    pub fn without_vaults(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self { vaults: BTreeMap::new(), access_policy }
    }

    /// A manager of the one vault `test`, of `provider` and granted to everyone, asking
    /// `access_policy` - for a test of what resolves a secret, which names it `test/KEY`.
    pub fn of_test_vault(
        provider: Arc<dyn SecretProvider>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        let vaults =
            BTreeMap::from([Self::test_vault(Self::TEST_VAULT, provider, Self::granted())]);
        Self { vaults, access_policy }
    }

    /// A manager of the vaults the kernel's tests use secrets of, asking `access_policy`: `test`,
    /// granted to everyone, `withheld`, the system's alone, and `bound`, granted to everyone but
    /// sent to `https://bound.example` alone, none of which has a key - for a test of in whose
    /// name a secret is used, whether it may be, and where it may go.
    #[expect(clippy::expect_used, reason = "test code: the origin is a literal")]
    pub fn of_test_vaults(access_policy: Arc<dyn AccessPolicy>) -> Self {
        use crate::testing::NoSecrets;
        let origin = Origin::new(Self::BOUND_ORIGIN).expect("an origin");
        let bound = VaultConfig { origins: Some(vec![origin]), ..Self::granted() };
        let vaults = BTreeMap::from([
            Self::test_vault(Self::TEST_VAULT, Arc::new(NoSecrets), Self::granted()),
            Self::test_vault(Self::WITHHELD_VAULT, Arc::new(NoSecrets), Self::withheld()),
            Self::test_vault(Self::BOUND_VAULT, Arc::new(NoSecrets), bound),
        ]);
        Self { vaults, access_policy }
    }

    /// What configures a vault of the tests besides its backend: the system's alone, its secrets
    /// sent anywhere.
    fn withheld() -> VaultConfig {
        use caelumora_domain::config::BackendConfig;
        VaultConfig::new(BackendConfig { kind: "test".to_string(), params: Default::default() })
    }

    /// What configures a vault of the tests granted to everyone.
    fn granted() -> VaultConfig {
        use caelumora_domain::security::{AclEntry, Subject};
        let everyone = AclEntry { subject: Subject::Everyone, grant: Grant::Read };
        VaultConfig { acl: vec![everyone], ..Self::withheld() }
    }

    /// The vault `name` of `provider`, configured as `config`.
    #[expect(clippy::expect_used, reason = "test code: the names of the vaults are literals")]
    fn test_vault(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        config: VaultConfig,
    ) -> (Identifier, Vault) {
        let name = Identifier::new(name).expect("the name of a vault of the tests");
        (name, Vault::new(provider, &config))
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
    async fn resolve(&self, secret: &SecretRef, sent_to: &str) -> SecretResult<String> {
        self.manager.resolve(&self.context, secret, Some(sent_to)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{context, user};
    use crate::{AllowAllPolicy, OwnershipPolicy};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::config::BackendConfig;
    use caelumora_domain::remote::{RemoteAuthConfig, RemoteEndpoint};
    use caelumora_domain::security::{AclEntry, Subject};
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

    /// What configures a vault besides its backend: falling back to `fallback` if given, the
    /// system's alone, its secrets sent anywhere.
    fn falling_back(fallback: Option<&str>) -> VaultConfig {
        let fallback = fallback.map(|f| Identifier::new(f).unwrap());
        VaultConfig { fallback, ..VaultConfig::new(BackendConfig::new("test", Value::Null)) }
    }

    /// The vault `name` of `provider`, configured as `config`.
    fn configured(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        config: VaultConfig,
    ) -> (Identifier, Vault) {
        (Identifier::new(name).unwrap(), Vault::new(provider, &config))
    }

    /// The vault `name` of `provider`, the system's alone, falling back to `fallback` if given.
    fn vault(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        fallback: Option<&str>,
    ) -> (Identifier, Vault) {
        configured(name, provider, falling_back(fallback))
    }

    /// The vault `name` of `provider`, configured to grant its use to `who`, falling back to
    /// `fallback` if given.
    fn granted(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        who: &[ActorId],
        fallback: Option<&str>,
    ) -> (Identifier, Vault) {
        let acl = who.iter().map(|w| AclEntry { subject: Subject::Actor(*w), grant: Grant::Read });
        configured(name, provider, VaultConfig { acl: acl.collect(), ..falling_back(fallback) })
    }

    /// The vault `name` of `provider`, its secrets sent to `origins` alone, falling back to
    /// `fallback` if given.
    fn bound(
        name: &str,
        provider: Arc<dyn SecretProvider>,
        origins: &[&str],
        fallback: Option<&str>,
    ) -> (Identifier, Vault) {
        let origins = origins.iter().map(|o| Origin::new(o).unwrap()).collect();
        configured(name, provider, VaultConfig { origins: Some(origins), ..falling_back(fallback) })
    }

    fn manager(
        vaults: impl IntoIterator<Item = (Identifier, Vault)>,
    ) -> ConfigResult<SecretManager> {
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
        assert_eq!(manager.resolve(&context, &secret("ops/KEY"), None).await.unwrap(), "ops'");
        assert_eq!(manager.resolve(&context, &secret("team/KEY"), None).await.unwrap(), "team's");
        let unknown = manager.resolve(&context, &secret("dev/KEY"), None).await.unwrap_err();
        assert!(matches!(unknown, SecretError::UnknownVault(s) if s.vault() == "dev"));
        let user = manager.resolve(&context, &secret("user/KEY"), None).await.unwrap_err();
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
        assert_eq!(manager.resolve(&context, &secret("local/OWN"), None).await.unwrap(), "local's");
        assert_eq!(
            manager.resolve(&context, &secret("local/SHARED"), None).await.unwrap(),
            "env's"
        );
        let missing = manager.resolve(&context, &secret("local/MISSING"), None).await.unwrap_err();
        assert!(matches!(missing, SecretError::NotFound(s) if s.as_str() == "local/MISSING"));
    }

    #[tokio::test]
    async fn a_failing_backend_fails_the_lookup_and_its_fallback_is_not_asked() {
        let manager = manager([
            vault("local", Arc::new(FailingSecretProvider), Some("env")),
            vault("env", holding(&[("KEY", "env's")]), None),
        ])
        .unwrap();
        let failed = manager.resolve(&SecurityContext::default(), &secret("local/KEY"), None).await;
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
    async fn a_vault_is_used_as_its_access_list_grants_and_its_fallback_only_once_granted_too() {
        let (bob, carol) = (user(), user());
        let vaults = [
            granted("team", holding(&[("OWN", "team's")]), &[bob, carol], Some("ops")),
            granted("ops", holding(&[("SHARED", "ops'")]), &[bob], None),
            vault("system", holding(&[("KEY", "the system's")]), Some("open")),
            granted("open", holding(&[("KEY", "open's")]), &[bob, carol], None),
        ];
        let manager =
            SecretManager::new(vaults.into_iter().collect(), Arc::new(OwnershipPolicy::new()))
                .unwrap();
        let read = |actor: ActorId, text: &str| {
            let manager = &manager;
            let secret = secret(text);
            async move { manager.resolve(&context(actor), &secret, None).await }
        };
        assert_eq!(read(carol, "team/OWN").await.unwrap(), "team's");
        assert_eq!(read(bob, "team/SHARED").await.unwrap(), "ops'", "granted on the way");
        let refused = read(carol, "team/SHARED").await;
        assert!(
            matches!(refused, Err(SecretError::PermissionDenied { .. })),
            "not hers on the way"
        );
        assert!(matches!(
            read(carol, "ops/SHARED").await,
            Err(SecretError::PermissionDenied { .. })
        ));
        let withheld = read(bob, "system/KEY").await;
        assert!(
            matches!(withheld, Err(SecretError::PermissionDenied { .. })),
            "nobody's but its own"
        );
        let past = read(carol, "system/OTHER").await;
        assert!(
            matches!(past, Err(SecretError::PermissionDenied { .. })),
            "a vault refused is not passed over to a fallback granted"
        );
        let system = manager.resolve(&SecurityContext::system(), &secret("system/KEY"), None).await;
        assert_eq!(system.unwrap(), "the system's");
    }

    #[tokio::test]
    async fn a_secret_is_sent_only_where_every_vault_of_the_way_lets_it_be_sent() {
        let api = ["https://api.example"];
        let manager = manager([
            bound("api", holding(&[("KEY", "api's")]), &api, None),
            vault("local", holding(&[]), Some("api")),
            bound("agents", holding(&[]), &api, Some("env")),
            vault("env", holding(&[("KEY", "env's")]), None),
            bound("broken", Arc::new(FailingSecretProvider), &api, None),
            bound("own", holding(&[("KEY", "own")]), &[], None),
        ])
        .unwrap();
        let read = |text: &str, sent_to: Option<&'static str>| {
            let manager = &manager;
            let secret = secret(text);
            async move { manager.resolve(&SecurityContext::default(), &secret, sent_to).await }
        };
        let refused_at = |result: SecretResult<String>| match result {
            Err(SecretError::NotSentTo { origin, .. }) => Some(origin.map(|o| o.to_string())),
            _ => None,
        };
        assert_eq!(read("api/KEY", Some("https://api.example/v1?q=1")).await.unwrap(), "api's");
        assert_eq!(read("api/KEY", None).await.unwrap(), "api's", "sent nowhere");
        let elsewhere = read("api/KEY", Some("https://user:pw@elsewhere.example/x")).await;
        let origin = Some(Some("https://elsewhere.example".to_string()));
        assert_eq!(refused_at(elsewhere), origin, "named by its origin alone");
        let no_url = read("api/KEY", Some("ftp://files.example/key")).await;
        assert_eq!(refused_at(no_url), Some(None), "no http or https URL");
        let past = read("local/KEY", Some("https://elsewhere.example")).await;
        assert!(refused_at(past).is_some(), "a fallback's origins hold on the way");
        let widened = read("agents/KEY", Some("https://elsewhere.example")).await;
        assert!(refused_at(widened).is_some(), "nor does a fallback widen them");
        let broken = read("broken/KEY", Some("https://elsewhere.example")).await;
        assert!(refused_at(broken).is_some(), "refused before its backend is asked");
        assert_eq!(read("own/KEY", None).await.unwrap(), "own", "no origins: the server's own");
        assert!(refused_at(read("own/KEY", Some("https://api.example")).await).is_some());

        // An endpoint's credential goes with its request, where that goes.
        let secrets = Arc::new(manager).for_context(SecurityContext::default());
        let endpoint = RemoteEndpoint {
            auth: Some(RemoteAuthConfig::Bearer { secret: secret("api/KEY") }),
            ..RemoteEndpoint::new("https://api.example/a2a")
        };
        let token = endpoint.bearer_token(&secrets, "https://api.example/v1").await.unwrap();
        assert_eq!(token.as_deref(), Some("api's"));
        let refused = endpoint.bearer_token(&secrets, "https://elsewhere.example/v1").await;
        assert!(matches!(refused, Err(SecretError::NotSentTo { .. })), "{refused:?}");
    }

    #[tokio::test]
    async fn access_policy_denial_blocks_lookup() {
        let vaults = [vault("local", holding(&[("KEY", "value")]), None)];
        let manager =
            SecretManager::new(vaults.into_iter().collect(), Arc::new(DenyAllPolicy)).unwrap();
        let err = manager
            .resolve(&SecurityContext::default(), &secret("local/KEY"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretError::PermissionDenied { .. }));
    }
}
