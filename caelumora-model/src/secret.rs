use crate::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found: {0}")]
    NotFound(String),
    #[error("access denied for secret: {0}")]
    PermissionDenied(String),
    #[error("backend: {0}")]
    Backend(String),
}

pub type SecretResult<T> = Result<T, SecretError>;

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
    security_manager: Arc<dyn SecurityManager>,
}

impl SecretManager {
    pub fn new(providers: Vec<Arc<dyn SecretProvider>>, security_manager: Arc<dyn SecurityManager>) -> Self {
        Self {
            providers,
            security_manager,
        }
    }

    pub async fn resolve(&self, context: &SecurityContext, key: &str) -> SecretResult<String> {
        self.security_manager.require(context, ActionType::ReadSecret(key), || SecretError::PermissionDenied(key.to_string())).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MapSecretProvider {
        entries: std::collections::HashMap<String, String>,
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
    struct AlwaysDenySecurityManager;

    #[async_trait]
    impl SecurityManager for AlwaysDenySecurityManager {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    fn dummy_context() -> SecurityContext {
        SecurityContext::default()
    }

    #[tokio::test]
    async fn falls_through_to_later_provider() {
        let mut first = std::collections::HashMap::new();
        first.insert("OTHER".to_string(), "unused".to_string());
        let mut second = std::collections::HashMap::new();
        second.insert("KEY".to_string(), "value-from-second".to_string());

        let manager = SecretManager::new(
            vec![
                Arc::new(MapSecretProvider { entries: first }),
                Arc::new(MapSecretProvider { entries: second }),
            ],
            Arc::new(crate::security::DummySecurityManager::new()),
        );

        let value = manager.resolve(&dummy_context(), "KEY").await.unwrap();
        assert_eq!(value, "value-from-second");
    }

    #[tokio::test]
    async fn failing_provider_does_not_abort_the_chain() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("KEY".to_string(), "value".to_string());

        let manager = SecretManager::new(
            vec![
                Arc::new(FailingSecretProvider),
                Arc::new(MapSecretProvider { entries }),
            ],
            Arc::new(crate::security::DummySecurityManager::new()),
        );

        let value = manager.resolve(&dummy_context(), "KEY").await.unwrap();
        assert_eq!(value, "value");
    }

    #[tokio::test]
    async fn not_found_when_no_provider_has_it() {
        let manager = SecretManager::new(
            vec![Arc::new(MapSecretProvider { entries: Default::default() })],
            Arc::new(crate::security::DummySecurityManager::new()),
        );

        let err = manager.resolve(&dummy_context(), "MISSING").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(_)));
    }

    #[tokio::test]
    async fn security_manager_denial_blocks_lookup() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("KEY".to_string(), "value".to_string());

        let manager = SecretManager::new(
            vec![Arc::new(MapSecretProvider { entries })],
            Arc::new(AlwaysDenySecurityManager),
        );

        let err = manager.resolve(&dummy_context(), "KEY").await.unwrap_err();
        assert!(matches!(err, SecretError::PermissionDenied(_)));
    }
}
