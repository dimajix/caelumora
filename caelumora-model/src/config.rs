use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::{Result};
use crate::security::{SecurityManager};
use serde::Deserialize;
use crate::store::{StateStore, EntityStore};
use crate::store_impl::json::JsonFileStore;
use crate::store_impl::memory::MemoryStore;

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

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AppConfig {
    pub persistence: Option<PersistenceConfig>,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn create_entity_store(&self, security_manager: Arc<dyn SecurityManager>) -> Result<Arc<dyn EntityStore>> {
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

    pub fn create_state_store(&self, security_manager: Arc<dyn SecurityManager>) -> Result<Arc<dyn StateStore>> {
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
}
