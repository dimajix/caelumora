//! The secrets of a process (issue 516): its sources, tried in order (`SecretProvider`), and the
//! `SecretManager` that asks the policy before each is read, in the name it is asked in. What is
//! read and how reading fails are the domain's (`caelumora_domain::secret`).

use crate::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use async_trait::async_trait;
use caelumora_domain::secret::{SecretError, SecretResult, Secrets};
use std::fmt::Debug;
use std::sync::Arc;

/// A single source of secrets, tried in order by a `SecretManager`.
///
/// `Ok(None)` means the key was not found in this provider and the next
/// provider in the chain should be tried; `Err` means this provider itself
/// malfunctioned (e.g. a corrupt secrets file), which is likewise not fatal
/// to the overall lookup and just falls through to the next provider.
#[async_trait]
pub trait SecretProvider: Debug + Sync + Send {
    async fn get_secret(&self, key: &str) -> SecretResult<Option<String>>;
}

#[derive(Debug)]
pub struct SecretManager {
    providers: Vec<Arc<dyn SecretProvider>>,
    access_policy: Arc<dyn AccessPolicy>,
}

impl SecretManager {
    pub fn new(
        providers: Vec<Arc<dyn SecretProvider>>,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        Self { providers, access_policy }
    }

    /// The secrets that may be read in the name of `context` (issue 516), each read asked of the
    /// policy then.
    pub fn for_context(self: &Arc<Self>, context: SecurityContext) -> SecretsFor {
        SecretsFor { manager: Arc::clone(self), context }
    }

    pub async fn resolve(&self, context: &SecurityContext, key: &str) -> SecretResult<String> {
        self.access_policy
            .require(context, ActionType::ReadSecret(key), |refused| {
                SecretError::PermissionDenied { key: key.to_string(), refused }
            })
            .await?;

        for provider in &self.providers {
            match provider.get_secret(key).await {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("Secret provider failed while looking up '{}': {}", key, e);
                    continue;
                }
            }
        }

        Err(SecretError::NotFound(key.to_string()))
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
    async fn resolve(&self, key: &str) -> SecretResult<String> {
        self.manager.resolve(&self.context, key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AllowAllPolicy;
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

    fn dummy_context() -> SecurityContext {
        SecurityContext::default()
    }

    #[tokio::test]
    async fn falls_through_to_later_provider() {
        let mut first = HashMap::new();
        first.insert("OTHER".to_string(), "unused".to_string());
        let mut second = HashMap::new();
        second.insert("KEY".to_string(), "value-from-second".to_string());

        let manager = SecretManager::new(
            vec![
                Arc::new(MapSecretProvider { entries: first }),
                Arc::new(MapSecretProvider { entries: second }),
            ],
            Arc::new(AllowAllPolicy::new()),
        );

        let value = manager.resolve(&dummy_context(), "KEY").await.unwrap();
        assert_eq!(value, "value-from-second");
    }

    #[tokio::test]
    async fn failing_provider_does_not_abort_the_chain() {
        let mut entries = HashMap::new();
        entries.insert("KEY".to_string(), "value".to_string());

        let manager = SecretManager::new(
            vec![Arc::new(FailingSecretProvider), Arc::new(MapSecretProvider { entries })],
            Arc::new(AllowAllPolicy::new()),
        );

        let value = manager.resolve(&dummy_context(), "KEY").await.unwrap();
        assert_eq!(value, "value");
    }

    #[tokio::test]
    async fn not_found_when_no_provider_has_it() {
        let manager = SecretManager::new(
            vec![Arc::new(MapSecretProvider { entries: Default::default() })],
            Arc::new(AllowAllPolicy::new()),
        );

        let err = manager.resolve(&dummy_context(), "MISSING").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(_)));
    }

    #[tokio::test]
    async fn access_policy_denial_blocks_lookup() {
        let mut entries = HashMap::new();
        entries.insert("KEY".to_string(), "value".to_string());

        let manager = SecretManager::new(
            vec![Arc::new(MapSecretProvider { entries })],
            Arc::new(DenyAllPolicy),
        );

        let err = manager.resolve(&dummy_context(), "KEY").await.unwrap_err();
        assert!(matches!(err, SecretError::PermissionDenied { .. }));
    }
}
