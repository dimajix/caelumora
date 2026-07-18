use crate::secret::SecretManager;
use crate::secret_impl::env::EnvSecretProvider;
use crate::secret_impl::file::FileSecretProvider;
use crate::security::SecurityManager;
use crate::store::{EntityStore, StateStore, StoreError};
use crate::store_impl::json::JsonFileStore;
use crate::store_impl::memory::MemoryStore;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Debug, Deserialize, Clone)]
pub struct JsonConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(untagged)]
pub enum PersistenceBackend {
    Json { json: JsonConfig },
    Memory { _dummy: Option<String> },
    #[default]
    Default,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PersistenceConfig {
    pub entity_store: Option<PersistenceBackend>,
    pub state_store: Option<PersistenceBackend>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FileSecretProviderConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretProviderConfig {
    File(FileSecretProviderConfig),
    Env,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecretsConfig {
    pub providers: Vec<SecretProviderConfig>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AppConfig {
    pub persistence: Option<PersistenceConfig>,
    pub secrets: Option<SecretsConfig>,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let content = fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn create_entity_store(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn EntityStore>> {
        let store : Arc<dyn EntityStore> = match &self.persistence {
            None => Arc::new(MemoryStore::new()),
            Some(persistence) => {
                match &persistence.entity_store {
                    Some(PersistenceBackend::Default) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Memory { _dummy}) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Json { json }) => Arc::new(JsonFileStore::new(&json.path, security_manager)?),
                    None => Arc::new(MemoryStore::new()),
                }
            }
        };
        Ok(store)
    }

    pub fn create_state_store(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<dyn StateStore>> {
        let store : Arc<dyn StateStore> = match &self.persistence {
            None => Arc::new(MemoryStore::new()),
            Some(persistence) => {
                match &persistence.state_store {
                    Some(PersistenceBackend::Default) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Memory { _dummy}) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Json { json }) => Arc::new(JsonFileStore::new(&json.path, security_manager)?),
                    None => Arc::new(MemoryStore::new()),
                }
            }
        };
        Ok(store)
    }

    pub fn create_secret_manager(&self, security_manager: Arc<dyn SecurityManager>) -> ConfigResult<Arc<SecretManager>> {
        let provider_configs: Vec<SecretProviderConfig> = match &self.secrets {
            Some(secrets) => secrets.providers.clone(),
            None => vec![
                SecretProviderConfig::File(FileSecretProviderConfig { path: PathBuf::from("local/secrets.json") }),
                SecretProviderConfig::Env,
            ],
        };

        let providers = provider_configs
            .into_iter()
            .map(|config| match config {
                SecretProviderConfig::File(file) => {
                    Arc::new(FileSecretProvider::new(file.path)) as Arc<dyn crate::secret::SecretProvider>
                }
                SecretProviderConfig::Env => Arc::new(EnvSecretProvider::new()) as Arc<dyn crate::secret::SecretProvider>,
            })
            .collect();

        Ok(Arc::new(SecretManager::new(providers, security_manager)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config() {
        let yaml = r#"
persistence:
  state_store:
    json:
      path: "local/state_store"
  template_store:
    json:
      path: "local/template_store"
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.persistence.is_some());
        let p = config.persistence.unwrap();
        match p.state_store {
            Some(PersistenceBackend::Json { json }) => assert_eq!(json.path, PathBuf::from("local/state_store")),
            _ => panic!("Expected Json backend"),
        }
    }

    #[test]
    fn test_load_config_memory() {
        let yaml = r#"
persistence:
  state_store:
    memory:
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.persistence.is_some());
        let p = config.persistence.unwrap();
        println!("Deserialized state_store: {:?}", p.state_store);
        match p.state_store {
            Some(PersistenceBackend::Memory { _dummy }) => (),
            _ => panic!("Expected Memory backend, got {:?}", p.state_store),
        }
    }

    #[test]
    fn test_load_config_none() {
        let yaml = r#"
persistence: {}
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.persistence.is_some());
        let p = config.persistence.unwrap();
        assert!(p.state_store.is_none());
    }

    #[test]
    fn test_load_config_secrets() {
        let yaml = r#"
secrets:
  providers:
    - type: file
      path: "local/secrets.json"
    - type: env
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        let secrets = config.secrets.expect("secrets config present");
        assert_eq!(secrets.providers.len(), 2);
        match &secrets.providers[0] {
            SecretProviderConfig::File(file) => assert_eq!(file.path, PathBuf::from("local/secrets.json")),
            _ => panic!("Expected File provider first"),
        }
        assert!(matches!(secrets.providers[1], SecretProviderConfig::Env));
    }

    #[tokio::test]
    async fn test_create_secret_manager_defaults_fall_through_to_env() {
        // With no `secrets` config, the default provider chain is
        // [File("local/secrets.json"), Env]. Since that file does not exist
        // in the test working directory, a lookup should fall through to
        // the env provider.
        std::env::set_var("CAELUMORA_TEST_CONFIG_SECRET", "value-from-env");
        let config = AppConfig::default();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let manager = config.create_secret_manager(security_manager).unwrap();
        let value = manager
            .resolve(&crate::security::SecurityContext::default(), "CAELUMORA_TEST_CONFIG_SECRET")
            .await
            .unwrap();
        assert_eq!(value, "value-from-env");
        std::env::remove_var("CAELUMORA_TEST_CONFIG_SECRET");
    }
}
