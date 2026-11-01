//! `EnvSecretProvider`: the secrets of the environment of the process.

use crate::backend::Backends;
use async_trait::async_trait;
use caelumora_api::component::NoParams;
use caelumora_model::secret::{SecretProvider, SecretResult};
use std::env;
use std::sync::Arc;

/// Resolves secrets from process environment variables.
#[derive(Debug, Default)]
pub struct EnvSecretProvider;

impl EnvSecretProvider {
    pub fn new() -> Self {
        Self
    }
}

/// Registers the secret provider `env` with `backends`.
pub(super) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn SecretProvider, _, _>("env", |_, _: NoParams, _| {
        Ok(Arc::new(EnvSecretProvider::new()))
    });
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
