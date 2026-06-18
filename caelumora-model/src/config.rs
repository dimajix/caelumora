use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::{Result};
use serde::Deserialize;
use crate::store::{StateStore, TemplateStore};
use crate::store_impl::json::JsonFileStore;
use crate::store_impl::memory::MemoryStore;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum PersistenceBackend {
    Memory,
    Json { path: PathBuf },
}

#[derive(Debug, Deserialize, Clone)]
pub struct PersistenceConfig {
    pub backend: Option<PersistenceBackend>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub persistence: Option<PersistenceConfig>,
}

impl AppConfig {
    pub fn default() -> Self {
        AppConfig {
            persistence: None
        }
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn create_template_store(&self) -> Result<Arc<dyn TemplateStore>> {
        let store : Arc<dyn TemplateStore> = match &self.persistence {
            None => Arc::new(MemoryStore::new()),
            Some(persistence) => {
                match &persistence.backend {
                    Some(PersistenceBackend::Memory) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Json { path }) => Arc::new(JsonFileStore::new(path)?),
                    None => Arc::new(MemoryStore::new()),
                }
            }
        };
        Ok(store)
    }

    pub fn create_state_store(&self) -> Result<Arc<dyn StateStore>> {
        let store : Arc<dyn StateStore> = match &self.persistence {
            None => Arc::new(MemoryStore::new()),
            Some(persistence) => {
                match &persistence.backend {
                    Some(PersistenceBackend::Memory) => Arc::new(MemoryStore::new()),
                    Some(PersistenceBackend::Json { path }) => Arc::new(JsonFileStore::new(path)?),
                    None => Arc::new(MemoryStore::new()),
                }
            }
        };
        Ok(store)
    }
}
