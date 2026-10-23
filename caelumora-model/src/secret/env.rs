use crate::secret::{SecretProvider, SecretResult};
use async_trait::async_trait;
use std::env;

/// Resolves secrets from process environment variables.
#[derive(Debug, Default)]
pub struct EnvSecretProvider;

impl EnvSecretProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecretProvider for EnvSecretProvider {
    async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
        Ok(env::var(key).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_existing_env_var() {
        env::set_var("CAELUMORA_TEST_SECRET_ENV", "hello");
        let provider = EnvSecretProvider::new();
        let value = provider.get_secret("CAELUMORA_TEST_SECRET_ENV").await.unwrap();
        assert_eq!(value, Some("hello".to_string()));
        env::remove_var("CAELUMORA_TEST_SECRET_ENV");
    }

    #[tokio::test]
    async fn returns_none_for_missing_var() {
        let provider = EnvSecretProvider::new();
        let value = provider.get_secret("CAELUMORA_TEST_SECRET_ENV_MISSING").await.unwrap();
        assert_eq!(value, None);
    }
}
