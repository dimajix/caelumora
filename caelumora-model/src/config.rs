use crate::command::default_commands;
use crate::component::ComponentConfig;
use crate::secret::SecretManager;
use crate::secret_impl::env::EnvSecretProvider;
use crate::secret_impl::file::FileSecretProvider;
use crate::security::SecurityManager;
use crate::store::{EntityStore, StateStore, StoreError};
use crate::store_impl::json::JsonFileStore;
use crate::store_impl::memory::MemoryStore;
use crate::user_provider::{UserManager, UserProvider};
use crate::user_provider_impl::accept_any::AcceptAnyUserProvider;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] serde_saphyr::Error),
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

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserProviderConfig {
    /// Development only: accepts any username and password (see
    /// `AcceptAnyUserProvider`). Never part of the default chain.
    AcceptAny,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UsersConfig {
    pub providers: Vec<UserProviderConfig>,
}

/// How the REST server keeps a browser logged in: a signed JWT in an
/// `HttpOnly` cookie, renewed while in use (issue 10).
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct AuthConfig {
    /// Name of the secret holding the JWT signing key, resolved through the
    /// secret chain. Also used as the token's `kid`, so rotating the key means
    /// pointing this at a new secret. If the secret does not exist, the server
    /// generates a random key at startup - which logs everybody out on every
    /// restart.
    pub jwt_secret: String,
    /// Lifetime of a token in seconds. A token is renewed once it is past half
    /// its lifetime; a session without requests for this long ends.
    pub token_lifetime_secs: u64,
    /// Absolute limit of a login in seconds, however active the user is.
    pub max_session_secs: u64,
    /// Whether the session cookie carries `Secure`. Browsers accept `Secure`
    /// cookies on `http://localhost`, so this only needs turning off for plain
    /// HTTP on another host - which should not happen outside a test setup.
    pub secure_cookie: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "CAELUMORA_JWT_SECRET".to_string(),
            token_lifetime_secs: 15 * 60,
            max_session_secs: 12 * 60 * 60,
            secure_cookie: true,
        }
    }
}

/// Format of log output.
#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Where and how a binary writes its log (issue 01). Which records are
/// written is controlled by `RUST_LOG`, not here.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct LoggingConfig {
    pub format: LogFormat,
    /// Log file to append to. Without one, each binary uses its own default:
    /// the REST server writes to stderr, the TUI to `local/tui.log` so the
    /// log does not interfere with its prompt.
    pub file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub persistence: Option<PersistenceConfig>,
    pub secrets: Option<SecretsConfig>,
    pub users: Option<UsersConfig>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// The user commands every session has (issue 28), as `CommandProvider`
    /// components; a session template can add its own. Defaults to `/help`
    /// and `/wakeup`; an empty `component_nodes` list switches them off.
    #[serde(default = "default_commands")]
    pub commands: ComponentConfig,
}

// Not derived: the commands default to the built-in ones, not to none.
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            persistence: None,
            secrets: None,
            users: None,
            auth: AuthConfig::default(),
            logging: LoggingConfig::default(),
            commands: default_commands(),
        }
    }
}

impl AppConfig {
    /// Reads and parses the configuration file at `path`.
    ///
    /// # Errors
    /// `ConfigError::Io` if the file cannot be read, `ConfigError::Parse` if
    /// it is not a valid configuration.
    pub fn load(path: impl AsRef<Path>) -> ConfigResult<Self> {
        let content = fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    /// Parses a configuration from YAML text.
    ///
    /// # Errors
    /// `ConfigError::Parse` if `yaml` is not a valid configuration; the error
    /// names the line and column.
    pub fn from_yaml(yaml: &str) -> ConfigResult<Self> {
        // By default the parser quotes the offending lines of the input in its
        // error, and a configuration may hold credentials. The location alone
        // is enough to find the mistake.
        let mut options = serde_saphyr::Options::default();
        options.with_snippet = false;
        Ok(serde_saphyr::from_str_with_options(yaml, options)?)
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

    /// Builds the chain of user providers from the `users` section. Without
    /// one the chain is empty and nobody can authenticate: there is no
    /// default provider, because the only one that needs no setup accepts
    /// everybody.
    pub fn create_user_manager(&self, entity_store: Arc<dyn EntityStore>) -> ConfigResult<Arc<UserManager>> {
        let provider_configs = self.users.as_ref()
            .map(|users| users.providers.clone())
            .unwrap_or_default();

        let providers = provider_configs
            .into_iter()
            .map(|config| match config {
                UserProviderConfig::AcceptAny => {
                    tracing::warn!("User provider 'accept_any' is enabled: ANY username and password will be accepted. \
                        Development only - never expose this server to a network.");
                    Arc::new(AcceptAnyUserProvider::new(entity_store.clone())) as Arc<dyn UserProvider>
                }
            })
            .collect();

        Ok(Arc::new(UserManager::new(providers)))
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
        let config = AppConfig::from_yaml(yaml).unwrap();
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
        let config = AppConfig::from_yaml(yaml).unwrap();
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
        let config = AppConfig::from_yaml(yaml).unwrap();
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
        let config = AppConfig::from_yaml(yaml).unwrap();
        let secrets = config.secrets.expect("secrets config present");
        assert_eq!(secrets.providers.len(), 2);
        match &secrets.providers[0] {
            SecretProviderConfig::File(file) => assert_eq!(file.path, PathBuf::from("local/secrets.json")),
            _ => panic!("Expected File provider first"),
        }
        assert!(matches!(secrets.providers[1], SecretProviderConfig::Env));
    }

    #[test]
    fn test_load_config_users() {
        let yaml = r#"
users:
  providers:
    - type: accept_any
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        let users = config.users.expect("users config present");
        assert_eq!(users.providers.len(), 1);
        assert!(matches!(users.providers[0], UserProviderConfig::AcceptAny));
    }

    #[test]
    fn test_load_config_logging() {
        let config = AppConfig::from_yaml("persistence: {}").unwrap();
        assert_eq!(config.logging.format, LogFormat::Text);
        assert!(config.logging.file.is_none());

        let yaml = r#"
logging:
  format: json
  file: "local/server.log"
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.logging.format, LogFormat::Json);
        assert_eq!(config.logging.file, Some(PathBuf::from("local/server.log")));
    }

    #[test]
    fn test_load_config_auth_defaults_and_overrides() {
        let config = AppConfig::from_yaml("persistence: {}").unwrap();
        assert_eq!(config.auth.token_lifetime_secs, 900);
        assert!(config.auth.secure_cookie);

        let yaml = r#"
auth:
  jwt_secret: "OTHER_KEY"
  token_lifetime_secs: 60
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.auth.jwt_secret, "OTHER_KEY");
        assert_eq!(config.auth.token_lifetime_secs, 60);
        assert_eq!(config.auth.max_session_secs, 12 * 60 * 60, "unset fields keep their defaults");
    }

    #[tokio::test]
    async fn test_create_user_manager_without_config_accepts_nobody() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let manager = AppConfig::default().create_user_manager(store).unwrap();

        let result = manager.authenticate("user", &crate::user_provider::Credential::Password("x")).await;
        assert!(result.is_err(), "no provider must be enabled by default");
    }

    #[tokio::test]
    async fn test_create_user_manager_with_accept_any() {
        let yaml = r#"
users:
  providers:
    - type: accept_any
"#;
        let config = AppConfig::from_yaml(yaml).unwrap();
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let manager = config.create_user_manager(store).unwrap();

        let user = manager.authenticate("alice", &crate::user_provider::Credential::Password("x")).await
            .expect("accept_any accepts everybody");
        assert_eq!(user.username, "alice");
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

    #[test]
    fn a_malformed_config_names_the_location_but_not_the_content() {
        let yaml = "auth:\n  secret_value_xyz: [unclosed\n";
        let err = AppConfig::from_yaml(yaml).expect_err("unclosed flow sequence must not parse");
        let message = err.to_string();
        assert!(message.contains("line"), "error should name the line: {message}");
        assert!(!message.contains("secret_value_xyz"), "error must not quote the input: {message}");
    }

    #[test]
    fn the_commands_default_to_the_builtin_ones_and_can_be_switched_off() {
        let expected = crate::command::default_commands().component_nodes.len();
        assert_eq!(AppConfig::default().commands.component_nodes.len(), expected);
        assert_eq!(AppConfig::from_yaml("persistence: {}").unwrap().commands.component_nodes.len(), expected);

        let config = AppConfig::from_yaml("commands:\n  component_nodes: []\n").unwrap();
        assert!(config.commands.component_nodes.is_empty(), "an explicit empty list means no global commands");
    }
}
