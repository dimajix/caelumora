use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum PersistenceBackend {
    Memory,
    Json { path: PathBuf },
}

#[derive(Debug, Deserialize, Clone)]
pub struct PersistenceConfig {
    pub backend: PersistenceBackend,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub persistence: PersistenceConfig,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }
}
