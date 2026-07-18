use crate::secret::{SecretError, SecretProvider, SecretResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

/// Resolves secrets from a flat JSON object file (`{"KEY": "value", ...}`).
///
/// The file is re-read on every lookup so secrets can be rotated without
/// restarting the process. A missing file is treated as "not found" rather
/// than an error, so this provider can be chained ahead of others without
/// requiring the file to exist up front.
#[derive(Debug)]
pub struct FileSecretProvider {
    path: PathBuf,
}

impl FileSecretProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait]
impl SecretProvider for FileSecretProvider {
    async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SecretError::Backend(e.to_string())),
        };

        let entries: HashMap<String, String> =
            serde_json::from_str(&content).map_err(|e| SecretError::Backend(e.to_string()))?;

        Ok(entries.get(key).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn reads_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, r#"{{"KEY": "value"}}"#).unwrap();

        let provider = FileSecretProvider::new(&path);
        let value = provider.get_secret("KEY").await.unwrap();
        assert_eq!(value, Some("value".to_string()));
    }

    #[tokio::test]
    async fn missing_key_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(&path, "{}").unwrap();

        let provider = FileSecretProvider::new(&path);
        let value = provider.get_secret("MISSING").await.unwrap();
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");

        let provider = FileSecretProvider::new(&path);
        let value = provider.get_secret("KEY").await.unwrap();
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn malformed_file_is_a_backend_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(&path, "not json").unwrap();

        let provider = FileSecretProvider::new(&path);
        let err = provider.get_secret("KEY").await.unwrap_err();
        assert!(matches!(err, SecretError::Backend(_)));
    }
}
